//! The `ultra9000` executable; the application lives in the `ultra9000_app` library. `ultra9000
//! ctl …` is the shell control link's client and never starts the app.

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|first| first == "ctl") {
        std::process::exit(ultra9000_app::ctl(args.collect()));
    }
    ultra9000_app::run();
}
