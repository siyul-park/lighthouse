mod cli;

fn main() {
    println!("{}", bins::api());
    cli::run();
}
