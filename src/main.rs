use clap::Parser;
use clap::Subcommand;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Add {
        url: String,
        path: String,
        #[arg(long)]
        depth: Option<u32>,
        #[arg(long)]
        sparse: Vec<String>,
    },
    Init,
    Status,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Add {
            url,
            path,
            depth,
            sparse,
        }) => {
            println!(
                "called picky add with url={url} path={path} depth={depth:?} sparse={sparse:?}"
            )
        }
        Some(Commands::Init) => {
            println!("called picky init")
        }
        Some(Commands::Status) => {
            println!("called picky status")
        }
        None => {
            println!("bare picky invocation")
        }
    }
}
