use std::borrow::Cow;
use std::{
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use anyhow::{anyhow, Result};
use env_logger::Env;
use futures::StreamExt;
use log::info;
use quinn::{ClientConfig, Endpoint, ServerConfig};

const LISTENING_PORT: u16 = 4000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    info!("Running test!");

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let c = testground::client::Client::new_and_init()
        .await
        .map_err(|e| anyhow!("{:?}", e))?;

    let seq_num = c.global_seq();

    match seq_num % 2 {
        0 => server(c).await?,
        1 => client(c).await?,
        _ => unreachable!(),
    }
    Ok(())
}

async fn server(client: testground::client::Client) -> Result<()> {
    let seq = client.global_seq();
    info!("building server {:?}", seq);

    let local_addr = match if_addrs::get_if_addrs()
        .unwrap()
        .into_iter()
        .find(|iface| iface.name == "eth1")
        .unwrap()
        .addr
        .ip()
    {
        IpAddr::V4(addr) => format!("{addr}:{LISTENING_PORT}"),
        IpAddr::V6(_) => unimplemented!(),
    };

    let server_task = tokio::spawn(run_server(local_addr.parse()?));
    let payload = serde_json::json!({
        "ID": seq,
        "Addrs": [local_addr],
    });

    client.publish("server", Cow::Owned(payload)).await?;

    info!("server {:?} built & waiting", seq);
    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;
    info!("{:?} done", seq);
    server_task.abort();
    Ok(())
}

async fn client(client: testground::client::Client) -> Result<()> {
    let seq = client.global_seq();
    info!("building client {:?}", seq);
    let local_addr = match if_addrs::get_if_addrs()
        .unwrap()
        .into_iter()
        .find(|iface| iface.name == "eth1")
        .unwrap()
        .addr
        .ip()
    {
        IpAddr::V4(addr) => format!("{addr}:{LISTENING_PORT}"),
        IpAddr::V6(_) => unimplemented!(),
    };
    info!("client addr {:?}", local_addr);
    let client_cfg = configure_client();
    let mut endpoint = Endpoint::client(local_addr.parse()?)?;
    endpoint.set_default_client_config(client_cfg);
    info!("client {:?} built", seq);

    let responders_count = client.run_parameters().test_instance_count as usize / 2;
    let mut responders_address_stream = client
        .subscribe("server", responders_count)
        .await
        .take(responders_count)
        .map(|a| {
            let value = a.unwrap();
            value["Addrs"][0].as_str().unwrap().to_string()
        });

    info!(" {:?} connecting to servers", seq);
    while let Some(addr) = responders_address_stream.next().await {
        let addr: SocketAddr = addr.parse()?;
        info!("request {:?} given address {:?} to dial", seq, addr);
        let connection = endpoint.connect(addr, "localhost").unwrap().await?;
        info!("[client] connected: addr={}", connection.remote_address());
    }

    drop(responders_address_stream);

    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;

    info!("{:?} done", seq);
    Ok(())
}

/// Runs a QUIC server bound to given address.
async fn run_server(addr: SocketAddr) {
    let (endpoint, _server_cert) = make_server_endpoint(addr).unwrap();
    // accept a single connection
    let incoming_conn = endpoint.accept().await.unwrap();
    let conn = incoming_conn.await.unwrap();
    info!(
        "[server] connection accepted: addr={}",
        conn.remote_address()
    );
}

async fn _run_client(server_addr: SocketAddr, addr: SocketAddr) -> Result<(), Box<dyn Error>> {
    let client_cfg = configure_client();
    let mut endpoint = Endpoint::client(addr)?;
    endpoint.set_default_client_config(client_cfg);

    // connect to server
    let connection = endpoint
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    info!("[client] connected: addr={}", connection.remote_address());
    // Dropping handles allows the corresponding objects to automatically shut down
    drop(connection);
    // Make sure the server has a chance to clean up
    endpoint.wait_idle().await;

    Ok(())
}

/// Dummy certificate verifier that treats any certificate as valid.
/// NOTE, such verification is vulnerable to MITM attacks, but convenient for testing.
struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

fn configure_client() -> ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();

    ClientConfig::new(Arc::new(crypto))
}

/// Constructs a QUIC endpoint configured to listen for incoming connections on a certain address
/// and port.
///
/// ## Returns
///
/// - a stream of incoming QUIC connections
/// - server certificate serialized into DER format
#[allow(unused)]
pub fn make_server_endpoint(bind_addr: SocketAddr) -> Result<(Endpoint, Vec<u8>), Box<dyn Error>> {
    let (server_config, server_cert) = configure_server()?;
    let endpoint = Endpoint::server(server_config, bind_addr)?;
    Ok((endpoint, server_cert))
}

/// Returns default server configuration along with its certificate.
#[allow(clippy::field_reassign_with_default)] // https://github.com/rust-lang/rust-clippy/issues/6527
fn configure_server() -> Result<(ServerConfig, Vec<u8>), Box<dyn Error>> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(priv_key);
    let cert_chain = vec![rustls::Certificate(cert_der.clone())];

    let mut server_config = ServerConfig::with_single_cert(cert_chain, priv_key)?;
    Arc::get_mut(&mut server_config.transport)
        .unwrap()
        .max_concurrent_uni_streams(0_u8.into());

    Ok((server_config, cert_der))
}

#[allow(unused)]
pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];
