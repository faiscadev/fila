pub struct Foo;

impl Foo {
    pub fn bar() -> &'static str {
        "Hello from Foo!"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        assert_eq!(Foo::bar(), "Hello from Foo!");
    }
}
