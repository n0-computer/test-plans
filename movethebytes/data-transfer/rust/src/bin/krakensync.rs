use std::borrow::Cow;
use std::{
    net::{IpAddr, SocketAddr},
    time::Instant,
};

use anyhow::{anyhow, Result};
use cid::Cid;
use env_logger::Env;
use futures::StreamExt;
use log::info;

use krakensync::{core::Store, network::Node};

#[tokio::main]
async fn main() -> Result<()> {
    run_test().await?;

    Ok(())
}

const LISTENING_PORT: u16 = 31131;

async fn run_test() -> Result<()> {
    info!("Running test!");

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let client = testground::client::Client::new_and_init()
        .await
        .map_err(|e| anyhow!("{:?}", e))?;

    let seq_num = client.global_seq();

    match seq_num % 2 {
        0 => responder(client).await?,
        1 => requester(client).await?,
        _ => unreachable!(),
    }
    Ok(())
}

async fn build_node(store: Store, addr: SocketAddr) -> Result<Node> {
    // let (server_config, server_cert) = krakensync::network::read_localhost_config()?;
    let (server_config, server_cert) = krakensync::network::configure_server()?;
    let node = Node::new(store, addr, server_config.clone(), server_cert.clone())?;
    Ok(node)
}

async fn responder(client: testground::client::Client) -> Result<()> {
    let seq = client.global_seq();
    info!("building responder {:?}", seq);

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

    let store = Store::default();
    let (cids, _) =
        krakensync::network::import_car_file(&store, "../../../fixtures/10MiB.car".to_string())
            .await?;
    if cids.len() != 1 {
        anyhow::bail!("expected /fixtures/10MiB.car file to have one root cid");
    }
    let cid = cids[0];
    let _node: Node = build_node(store, local_addr.parse()?).await?;

    let payload = serde_json::json!({
        "ID": seq,
        "Addrs": [local_addr],
        "Cid": cid.to_string(),
    });

    client.publish("responders", Cow::Owned(payload)).await?;

    info!("responder {:?} built & waiting", seq);
    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;
    info!("{:?} done", seq);
    Ok(())
}

async fn requester(client: testground::client::Client) -> Result<()> {
    let seq = client.global_seq();
    info!("building requester {:?}", seq);

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

    let store = Store::default();
    let mut node: Node = build_node(store, local_addr.parse()?).await?;

    info!("requester {:?} built with addr {:?}", seq, local_addr);
    let responders_count = client.run_parameters().test_instance_count as usize / 2;
    let mut cids = vec![];
    let mut responders_address_stream = client
        .subscribe("responders", responders_count)
        .await
        .take(responders_count)
        .map(|a| {
            let value = a.unwrap();
            let cid = value["Cid"].as_str().unwrap().to_string();
            cids.push(cid);
            value["Addrs"][0].as_str().unwrap().to_string()
        });

    info!("requester {:?} connecting to responders", seq);
    while let Some(addr) = responders_address_stream.next().await {
        let addr: SocketAddr = addr.parse()?;
        info!("request {:?} given address {:?} to dial", seq, addr);
        node.insecure_connect(addr).await?;
    }

    drop(responders_address_stream);

    info!("requester {:?} fetching data", seq);
    for cid in cids {
        let t0 = Instant::now();
        info!("fetching CID {:?}", cid);
        let cid: Cid = cid.parse()?;
        let mut updates = node.sync(krakensync::Query::new(cid));
        if let Some(update) = updates.next().await {
            update?;
            info!(
                "node {:?} - cid {:?} - REQUESTER_TTFB: {:?}",
                seq,
                cid,
                t0.elapsed().as_secs_f64()
            );
        }

        while let Some(update) = updates.next().await {
            let (_have, _total) = update?;
        }

        info!(
            "node {:?} - cid {:?} - REQUEST_FETCH_DURATION: {:?}",
            seq,
            cid,
            t0.elapsed().as_secs_f64()
        );
    }
    info!("requester {:?} finished and waiting", seq);
    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;

    info!("{:?} done", seq);
    Ok(())
}
