pub struct Bar;

impl Bar {
    pub fn baz() -> &'static str {
        "Hello from Bar!"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        assert_eq!(Bar::baz(), "Hello from Bar!");
    }
}
