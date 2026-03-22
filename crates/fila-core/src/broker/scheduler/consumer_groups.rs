use std::collections::HashMap;

/// Key for a consumer group: (queue_id, group_name).
type GroupKey = (String, String);

/// Tracks membership and round-robin state for a single consumer group.
struct GroupState {
    /// Ordered list of consumer IDs in this group. The order determines
    /// round-robin delivery. New members are appended at the end.
    members: Vec<String>,
    /// Round-robin index for this group. Points to the next member that
    /// should receive a message.
    rr_idx: usize,
}

/// Manages consumer group membership and per-group round-robin delivery.
///
/// A consumer group allows multiple consumers to share message delivery
/// for a queue: each message goes to exactly one member of the group.
/// Consumers without a group operate independently (classic behavior).
pub(super) struct ConsumerGroupManager {
    /// Map from (queue_id, group_name) to the group's state.
    groups: HashMap<GroupKey, GroupState>,
    /// Reverse index: consumer_id → (queue_id, group_name) for O(1) removal.
    consumer_to_group: HashMap<String, GroupKey>,
}

impl ConsumerGroupManager {
    pub(super) fn new() -> Self {
        Self {
            groups: HashMap::new(),
            consumer_to_group: HashMap::new(),
        }
    }

    /// Add a consumer to a group. If the group doesn't exist, it is created.
    pub(super) fn join(&mut self, queue_id: &str, group_name: &str, consumer_id: &str) {
        let key = (queue_id.to_string(), group_name.to_string());
        self.consumer_to_group
            .insert(consumer_id.to_string(), key.clone());
        let state = self.groups.entry(key).or_insert_with(|| GroupState {
            members: Vec::new(),
            rr_idx: 0,
        });
        // Avoid duplicate membership (idempotent join)
        if !state.members.contains(&consumer_id.to_string()) {
            state.members.push(consumer_id.to_string());
        }
    }

    /// Remove a consumer from its group. If the group becomes empty, it is removed.
    /// Returns the (queue_id, group_name) if the consumer was in a group.
    pub(super) fn leave(&mut self, consumer_id: &str) -> Option<GroupKey> {
        let key = self.consumer_to_group.remove(consumer_id)?;
        if let Some(state) = self.groups.get_mut(&key) {
            state.members.retain(|id| id != consumer_id);
            // Adjust rr_idx if it's past the end
            if !state.members.is_empty() {
                state.rr_idx %= state.members.len();
            }
            if state.members.is_empty() {
                self.groups.remove(&key);
            }
        }
        Some(key)
    }

    /// Check if a consumer is in a group.
    pub(super) fn is_in_group(&self, consumer_id: &str) -> bool {
        self.consumer_to_group.contains_key(consumer_id)
    }

    /// Get all groups, optionally filtered by queue.
    pub(super) fn all_groups(
        &self,
        queue_filter: Option<&str>,
    ) -> Vec<crate::broker::command::ConsumerGroupInfo> {
        self.groups
            .iter()
            .filter(|((qid, _), _)| queue_filter.map_or(true, |filter| qid == filter))
            .map(
                |((qid, name), state)| crate::broker::command::ConsumerGroupInfo {
                    queue: qid.clone(),
                    group_name: name.clone(),
                    member_count: state.members.len() as u32,
                    members: state.members.clone(),
                },
            )
            .collect()
    }

    /// Pick the next consumer in the group via round-robin.
    /// Returns `None` if the group has no members.
    pub(super) fn next_member(&mut self, queue_id: &str, group_name: &str) -> Option<String> {
        let key = (queue_id.to_string(), group_name.to_string());
        let state = self.groups.get_mut(&key)?;
        if state.members.is_empty() {
            return None;
        }
        let idx = state.rr_idx % state.members.len();
        let consumer_id = state.members[idx].clone();
        state.rr_idx = idx.wrapping_add(1);
        Some(consumer_id)
    }

    /// Get the list of member consumer IDs for a group.
    pub(super) fn group_members(&self, queue_id: &str, group_name: &str) -> Option<&[String]> {
        let key = (queue_id.to_string(), group_name.to_string());
        self.groups.get(&key).map(|s| s.members.as_slice())
    }

    /// Remove all consumers for a queue (used on leader loss / drop_queue_consumers).
    pub(super) fn remove_queue(&mut self, queue_id: &str) {
        let keys_to_remove: Vec<GroupKey> = self
            .groups
            .keys()
            .filter(|(qid, _)| qid == queue_id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(state) = self.groups.remove(&key) {
                for consumer_id in &state.members {
                    self.consumer_to_group.remove(consumer_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_and_leave() {
        let mut mgr = ConsumerGroupManager::new();
        mgr.join("q1", "grp1", "c1");
        mgr.join("q1", "grp1", "c2");
        assert!(mgr.is_in_group("c1"));
        assert!(mgr.is_in_group("c2"));
        assert!(!mgr.is_in_group("c3"));

        assert_eq!(mgr.group_members("q1", "grp1").unwrap().len(), 2);

        // Leave
        let key = mgr.leave("c1");
        assert_eq!(key, Some(("q1".to_string(), "grp1".to_string())));
        assert!(!mgr.is_in_group("c1"));
        assert_eq!(mgr.group_members("q1", "grp1").unwrap().len(), 1);

        // Leave last member removes group
        mgr.leave("c2");
        assert!(mgr.group_members("q1", "grp1").is_none());
    }

    #[test]
    fn round_robin_distribution() {
        let mut mgr = ConsumerGroupManager::new();
        mgr.join("q1", "grp1", "c1");
        mgr.join("q1", "grp1", "c2");
        mgr.join("q1", "grp1", "c3");

        let m1 = mgr.next_member("q1", "grp1").unwrap();
        let m2 = mgr.next_member("q1", "grp1").unwrap();
        let m3 = mgr.next_member("q1", "grp1").unwrap();
        let m4 = mgr.next_member("q1", "grp1").unwrap();

        assert_eq!(m1, "c1");
        assert_eq!(m2, "c2");
        assert_eq!(m3, "c3");
        assert_eq!(m4, "c1"); // wraps around
    }

    #[test]
    fn idempotent_join() {
        let mut mgr = ConsumerGroupManager::new();
        mgr.join("q1", "grp1", "c1");
        mgr.join("q1", "grp1", "c1"); // duplicate
        assert_eq!(mgr.group_members("q1", "grp1").unwrap().len(), 1);
    }

    #[test]
    fn remove_queue_clears_all_groups() {
        let mut mgr = ConsumerGroupManager::new();
        mgr.join("q1", "grp1", "c1");
        mgr.join("q1", "grp2", "c2");
        mgr.join("q2", "grp1", "c3");

        mgr.remove_queue("q1");
        assert!(!mgr.is_in_group("c1"));
        assert!(!mgr.is_in_group("c2"));
        assert!(mgr.is_in_group("c3")); // q2 untouched
    }

    #[test]
    fn all_groups_with_filter() {
        let mut mgr = ConsumerGroupManager::new();
        mgr.join("q1", "grp1", "c1");
        mgr.join("q2", "grp2", "c2");

        let all = mgr.all_groups(None);
        assert_eq!(all.len(), 2);

        let q1_only = mgr.all_groups(Some("q1"));
        assert_eq!(q1_only.len(), 1);
        assert_eq!(q1_only[0].queue, "q1");
    }
}
