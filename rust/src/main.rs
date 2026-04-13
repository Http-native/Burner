mod cli;
mod remote;
mod runtime;
mod service;
mod store;
mod ui;

const BANNER: &str = r"  
  ___                           
 | _ ) _  _  _ _  _ _   ___  _ _ 
 | _ \| || || '_|| ' \ / -_)| '_|Copyright (c) 2025-present Native people labs.
 |___/ \_,_||_|  |_||_|\___||_|  Engine: 1-dep-rs
";

fn main() {
    println!("{BANNER}");
    if let Err(err) = cli::run(std::env::args().skip(1).collect()) {
        ui::print_error_block(&err.to_string());
        std::process::exit(1);
    }
}
