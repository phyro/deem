//! The `deem` binary: the command line, a client of the `deem` library.

mod cli;

fn main() {
    std::process::exit(cli::main())
}
