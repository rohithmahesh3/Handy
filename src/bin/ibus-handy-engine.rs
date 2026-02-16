use clap::Parser;
use log::info;

use handy_app_lib::ibus_engine::{create_context, init, run_main_loop};

#[derive(Debug, clap::Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[clap(long)]
    ibus: bool,

    #[clap(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    info!("Starting Handy IBus Engine");

    let context = create_context();
    init(&context, args.ibus);

    info!("Entering IBus main loop");
    run_main_loop();

    Ok(())
}
