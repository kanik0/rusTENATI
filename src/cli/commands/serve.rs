use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, default_value = "8080")]
    pub port: u16,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Open browser automatically
    #[arg(long)]
    pub open: bool,
}

pub async fn run(args: &ServeArgs) -> Result<()> {
    rustenati::web::start_server(&args.bind, args.port, args.open).await
}
