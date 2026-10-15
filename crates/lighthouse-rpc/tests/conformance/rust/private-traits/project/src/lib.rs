trait Greet {
    fn hello(&self) -> String;
    fn bye(&self) -> String;
}

struct English;

impl Greet for English {
    fn hello(&self) -> String {
        "hello".to_owned()
    }

    fn bye(&self) -> String {
        "bye".to_owned()
    }
}
