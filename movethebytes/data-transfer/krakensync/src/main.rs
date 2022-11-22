use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};

use anyhow::{anyhow, Context, Result};
use cid::Cid;
use env_logger::Env;
use futures::StreamExt;
use log::info;
use testground::network_conf::{
    FilterAction, LinkShape, NetworkConfiguration, RoutingPolicyType, DEFAULT_DATA_NETWORK,
};

use krakensync::{core::Store, network::Node};

#[tokio::main]
async fn main() -> Result<()> {
    run_test().await?;

    Ok(())
}

const LISTENING_PORT: u16 = 31131;

async fn run_test() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    info!("Running test!");
    let client = testground::client::Client::new_and_init()
        .await
        .map_err(|e| anyhow!("{:?}", e))?;

    let seq_num = client.global_seq();

    configure_network(&client).await?;

    match seq_num % 2 {
        0 => responder(client).await?,
        1 => requester(client).await?,
        _ => unreachable!(),
    }
    Ok(())
}

async fn configure_network(client: &testground::client::Client) -> Result<()> {
    let latency_ms: u64 = client
        .run_parameters()
        .test_instance_params
        .get("latency")
        .context("error getting latency from testground")?
        .parse()?;

    let bandwidth: u64 = client
        .run_parameters()
        .test_instance_params
        .get("bandwidth")
        .context("error getting bandwith from testground")?
        .parse()?;

    let latency = std::time::Duration::from_millis(latency_ms)
        .as_nanos()
        .try_into()?;

    let network_conf = NetworkConfiguration {
        network: DEFAULT_DATA_NETWORK.to_owned(),
        ipv4: None,
        ipv6: None,
        enable: true,
        default: LinkShape {
            latency,
            jitter: 0,
            bandwidth,
            filter: FilterAction::Accept,
            loss: 0.0,
            corrupt: 0.0,
            corrupt_corr: 0.0,
            reorder: 0.0,
            reorder_corr: 0.0,
            duplicate: 0.0,
            duplicate_corr: 0.0,
        },
        rules: None,
        callback_state: format!("network-configured"),
        callback_target: Some(client.run_parameters().test_instance_count),
        routing_policy: RoutingPolicyType::AllowAll,
    };

    client.configure_network(network_conf).await?;
    Ok(())
}

async fn build_node(store: Store, addr: SocketAddr) -> Result<Node> {
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
        krakensync::network::import_car_file(&store, "/fixtures/10MiB.car".to_string()).await?;
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
    client.record_success().await?;
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
        info!("fetching CID {:?}", cid);
        let cid: Cid = cid.parse()?;
        let mut updates = node.sync(krakensync::Query::new(cid));
        if let Some(update) = updates.next().await {
            update?;
        }

        while let Some(update) = updates.next().await {
            let (_have, _total) = update?;
        }
    }
    info!("requester {:?} finished and waiting", seq);
    client
        .signal_and_wait(
            "done".to_string(),
            client.run_parameters().test_instance_count,
        )
        .await?;

    info!("{:?} done", seq);
    client.record_success().await?;
    Ok(())
}
