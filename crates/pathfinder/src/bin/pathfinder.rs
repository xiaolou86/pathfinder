use pathfinder_lib::{cairo, config, rpc, sequencer, storage::Storage};
use tracing::info;

#[tokio::main]
async fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt::init();

    info!("🏁 Starting node.");
    let config =
        config::Configuration::parse_cmd_line_and_cfg_file().expect("Configuration failed");

    // TODO: get database path from configuration
    let storage = Storage::migrate("database.sqlite".into()).unwrap();
    // TODO: pass the correct value from ethereum::chain.
    let sequencer = sequencer::Client::new(pathfinder_lib::ethereum::Chain::Goerli).unwrap();

    // TODO: the error could be recovered, but currently it's required for startup. There should
    // not be other reason for the start to fail than python script not firing up.
    let (call_handle, _jh) = cairo::ext_py::start(
        storage.path().into(),
        std::num::NonZeroUsize::new(2).unwrap(),
        futures::future::pending(),
    )
    .await
    .unwrap();

    let api = rpc::api::RpcApi::new(storage, sequencer, pathfinder_lib::ethereum::Chain::Goerli)
        .with_call_handling(call_handle);

    let (_handle, local_addr) =
        rpc::run_server(config.http_rpc_addr, api).expect("⚠️ Failed to start HTTP-RPC server");
    info!("📡 HTTP-RPC server started on: {}", local_addr);
    let () = std::future::pending().await;
}
