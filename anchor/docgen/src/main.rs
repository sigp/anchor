use std::process;

use clap::Parser;
use docgen::{anchor_command, interface::DocGen, render_docs};

fn main() {
    let args = DocGen::parse();
    let mut cmd = anchor_command();
    let result = render_docs(args.command, &mut cmd);

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
