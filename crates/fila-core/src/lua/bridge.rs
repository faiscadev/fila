use std::sync::Arc;

use mlua::Lua;

use crate::storage::Storage;

/// Register the `fila` namespace table with `fila.get(key)` that reads
/// runtime config from the storage `state` column family.
///
/// The storage reference is captured by the closure and used for each
/// `fila.get()` call. Since Lua runs on the scheduler thread which owns
/// storage access, this is safe without additional synchronization.
pub fn register_fila_api(lua: &Lua, storage: Arc<dyn Storage>) -> mlua::Result<()> {
    let fila_table = lua.create_table()?;

    let get_fn = lua.create_function(move |_, key: String| {
        match storage.get_state(&key) {
            Ok(Some(bytes)) => {
                // Return as string — Lua scripts receive string values
                let value = String::from_utf8_lossy(&bytes).into_owned();
                Ok(Some(value))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                // Log the error but return nil to Lua — don't crash the script
                tracing::warn!(key = %key, error = %e, "fila.get() storage error");
                Ok(None)
            }
        }
    })?;

    fila_table.set("get", get_fn)?;
    lua.globals().set("fila", fila_table)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::RocksDbStorage;

    #[test]
    fn fila_get_reads_from_state_cf() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Pre-populate state CF
        storage.put_state("max_retries", b"3").unwrap();
        storage
            .put_state("queue_name", b"orders")
            .unwrap();

        let lua = crate::lua::sandbox::create_sandbox().unwrap();
        register_fila_api(&lua, storage).unwrap();

        // Existing key returns value
        let result: String = lua
            .load(r#"return fila.get("queue_name")"#)
            .eval()
            .expect("fila.get existing key");
        assert_eq!(result, "orders");

        // Missing key returns nil
        let result: mlua::Value = lua
            .load(r#"return fila.get("nonexistent")"#)
            .eval()
            .expect("fila.get missing key");
        assert!(matches!(result, mlua::Value::Nil));

        // Use in Lua expression
        let retries: i64 = lua
            .load(r#"return tonumber(fila.get("max_retries")) + 1"#)
            .eval()
            .expect("fila.get in expression");
        assert_eq!(retries, 4);
    }
}
