use std::borrow::Cow;
use std::net::{TcpListener, TcpStream};

use anyhow::Result;
use env_logger::Env;
use futures::StreamExt;
use log::info;

const LISTENING_PORT: u16 = 1234;

#[tokio::main]
async fn main() -> Result<()> {
    run_tcp_test().await?;

    Ok(())
}

async fn run_tcp_test() -> Result<()> {
    info!("Running test!");
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let c = testground::client::Client::new_and_init().await.unwrap();
    let seq_num = c.global_seq();

    match seq_num % 2 {
        0 => server(c).await?,
        1 => client(c).await?,
        _ => unreachable!(),
    };
    Ok(())
}

async fn client(client: testground::client::Client) -> Result<()> {
    let server_count = client.run_parameters().test_instance_count as usize / 2;
    let mut server_address_stream = client
        .subscribe("server", server_count)
        .await
        .take(server_count)
        .map(|a| {
            let value = a.unwrap();
            value["Addrs"][0].as_str().unwrap().to_string()
        });

    while let Some(addr) = server_address_stream.next().await {
        let addr: std::net::SocketAddr = addr.parse()?;
        info!("Dialing {:?}", addr);
        TcpStream::connect(addr)?;
        info!("connected!");
    }

    drop(server_address_stream);

    client.signal("done").await?;
    client.record_success().await?;
    Ok(())
}

async fn run_server(listener: TcpListener) {
    for stream in listener.incoming() {
        stream.unwrap();
        info!("connection established");
    }
}

async fn server(client: testground::client::Client) -> Result<()> {
    let seq = client.global_seq();
    let local_addr = get_local_addr().await?;

    let listener = TcpListener::bind(local_addr.clone())?;
    let server_task = tokio::spawn(async move { run_server(listener).await });
    info!(
        "TCP server, listening for incoming connections on: {:?}.",
        local_addr
    );

    let payload = serde_json::json!({
        "ID": seq,
        "Addrs": [local_addr],
    });

    client.publish("server", Cow::Owned(payload)).await?;

    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;
    info!("done");
    server_task.abort();
    client.record_success().await?;
    Ok(())
}

async fn get_local_addr() -> Result<String> {
    match if_addrs::get_if_addrs()
        .unwrap()
        .into_iter()
        .find(|iface| iface.name == "eth1")
        .unwrap()
        .addr
        .ip()
    {
        std::net::IpAddr::V4(addr) => Ok(format!("{addr}:{LISTENING_PORT}")),
        std::net::IpAddr::V6(_) => unimplemented!(),
    }
}
