fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().is_some_and(|a| a == "native-event") {
        kelpserver::native::hooks::run(&args[1..])
    } else if args.is_empty() {
        println!("{}", kelpserver::native::cli::HELP);
        Ok(())
    } else {
        kelpserver::native::cli::run(&args)
    };
    if let Err(error) = result {
        eprintln!("kelpserver: {error}");
        std::process::exit(1);
    }
}
