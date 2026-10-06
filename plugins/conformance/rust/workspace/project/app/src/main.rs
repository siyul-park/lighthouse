use core_lib::Thing;

fn main() {
    let thing = renamed::make();
    thing.size();
    let _: Option<Thing> = None;
}
