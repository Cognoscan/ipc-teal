
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    ipc_path: String,

    #[arg(short, long)]
    recv: bool,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let args = Args::parse();

    if args.recv {
        recv(&args.ipc_path).await
    } else {
        send(&args.ipc_path).await
    }
}

async fn send(path: &str) -> color_eyre::Result<()> {
    let _sender = ipc_teal::Broadcaster::new(path, 1<<24, 1024)?;
    tokio::signal::ctrl_c().await?;
    println!("Shutting Down normally");
    Ok(())
}

async fn recv(path: &str) -> color_eyre::Result<()> {
    let _recv = ipc_teal::BroadReceiver::open(path).await?;
    println!("Connected!");
    Ok(())
}
