use fila_broker::Foo;
use fila_codec::Bar;

fn main() {
    println!("{}", Foo::bar());
    println!("{}", Bar::baz());

    println!("Hello, world!");
}
