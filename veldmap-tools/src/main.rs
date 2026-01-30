mod gui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use veldmap_geo_math::create_geo_math;
use gui::VeldMapToolsGui;
use log::{info, error};
use std::fs::File;
use std::os::unix::io::AsRawFd;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the world dataset (Low Res GLO-90)
    InitWorld {
        /// Path to the data directory (root of the pyramid)
        #[arg(short, long, default_value = "./data/dem")]
        output_dir: PathBuf,
    },
    /// Download a high-res region (GLO-30)
    AddRegion {
        /// Bounding box in format "min_lon,min_lat,max_lon,max_lat"
        /// Example for European Russia: "26,41,60,70"
        #[arg(long)]
        bbox: String,
        
        /// Path to the data directory
        #[arg(short, long, default_value = "./data/dem")]
        output_dir: PathBuf,
    },
}

fn redirect_stdio(log_file: &PathBuf) -> anyhow::Result<()> {
    let file = File::create(log_file)?;
    let fd = file.as_raw_fd();
    unsafe {
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
    }
    Ok(())
}

fn setup_logger() -> Result<(), fern::InitError> {
    let cache_dir = PathBuf::from("cache");
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)?;
    }
    let log_path = cache_dir.join("veldmap-tools.log");

    let _ = redirect_stdio(&log_path);

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .level_for("aws_config", log::LevelFilter::Warn)
        .level_for("aws_sdk_s3", log::LevelFilter::Warn)
        .level_for("wgpu_core", log::LevelFilter::Error)
        .level_for("wgpu_hal", log::LevelFilter::Error)
        .level_for("naga", log::LevelFilter::Error)
        .level_for("iced_wgpu", log::LevelFilter::Error)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    setup_logger().ok();
    
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() <= 1 {
        info!("Starting GUI mode");
        std::env::set_var("WGPU_BACKEND", "vulkan");

        let emoji_font_data = include_bytes!("../../assets/NotoColorEmoji.ttf");

        return iced::application("VeldMap Tools", VeldMapToolsGui::update, VeldMapToolsGui::view)
            .theme(VeldMapToolsGui::theme)
            .font(emoji_font_data)
            .run_with(VeldMapToolsGui::new)
            .map_err(|e| {
                error!("GUI error: {}", e);
                anyhow::anyhow!("GUI error: {}", e)
            });
    }

    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        handle_cli(cli).await
    })
}

async fn handle_cli(cli: Cli) -> anyhow::Result<()> {
    let _geo_math = create_geo_math();

    if let Some(command) = cli.command {
        match command {
            Commands::InitWorld { output_dir: _ } => {
                info!("InitWorld command received");
            }
            Commands::AddRegion { bbox, output_dir: _ } => {
                info!("AddRegion command received for bbox: {}", bbox);
                // CLI download logic is currently disabled during refactoring
                info!("CLI download is temporarily disabled. Please use GUI.");
            }
        }
    }

    Ok(())
}
