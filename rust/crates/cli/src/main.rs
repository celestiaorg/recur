use clap::Parser;

#[derive(Parser)]
#[command(
    name = "recur",
    version,
    about = "Open paths in Recur and author Recur Bundles"
)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    Cli::parse();
    Ok(())
}
