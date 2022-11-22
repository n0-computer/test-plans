# So you wanna write a test plan in rust for the Move The Bytes Working Group

So you wanna write a test plan in rust for the Move The Bytes Working Group that tests your protocol? Here is a guide! Note: this guide assumes you already have testground installed. The purpose of this guide is to make it quicker to onboard yourself into the wonderful world of writing a test plan. 

It is subject to change as we adjust our expectations for the Working Group and learn more about testground.

A few things to note:
- testground is currently not working on mac M1 architecture, so you either need to have some cloud infra you can access or a linux box at home. Or Windows I guess but that is uncharted territory GLHF
- the dev cycle is painful. You can short circuit some issues by running `cargo run --bin MY_PLAN` to ensure it builds before running it on testground. Note: the test will panic when you run it like this, that is expected.
- log log log. log a lot and often when you are starting to write your test plan. Logs are the only way you can get insight into your test at this stage, and the cycle to add a log and run the test again is __long__. Just err on the side of logging too much & remove everything unnecessary later.

This document is written with the `github.com/n0-computer/test-plans` repository in mind.

#### How do I set up my test plan?
Your test plan should be written as a new crate in the `data-transfer` folder:
`test-plan/movethebytes/data-transfer/MY_PLAN`

Besides all the other normal rust crate goodies, you need a `manifest.toml`, `_compositions/MY_PLAN.toml` and a `DockerFile`. You can use those corresponding files in the `test-plan/movethebytes/data-transfer/krakensync` example as a basis.

Note that there is a distinction between a `plan` and a `testcase`. In the `krakensync` example, `krakensync` is the plan and `one_to_one` is the testcase. This is referenced in both the `manifest.toml` and `_compositions/MY_PLAN.toml` files.

The `DockerFile` expects a single binary inside the test plan crate. For an example with multiple binaries per crate, take a look at `test-plan/movethebytes/data-transfer/rust`, and the corresponding `_compositions/tcp.toml` and `DockerFile` files.

The `krakensync` `DockerFile` also shows an example of how to add test fixture files to the docker container.

Ensure the `_compositions/MY_PLAN.toml` file contains the correct builder ("docker:generic") and correct runner ("local:docker").

Your crate should be structured like this:
```
MY_PLAN
  _compositions/MY_PLAN.toml
  fixtures/
  src/
  Cargo.lock
  Cargo.toml
  DockerFile
  manifest.toml
```

#### How do I add the test plan to testground?
    - `testground plan import --from test-plan/movethebytes/data-transfer/MY_PLAN`
To check if your plan was imported:
    - `testground plan list --testcases | grep MY_PLAN` 

#### How do I run the test plan?
First, you need `testground daemon` running in one terminal.
Then call `testground run composition --file PATH/TO/COMPOSITION/FILE`.

#### more notes on logging
Testground will output any logs you add in the testplan and in your protocol, as long as you set up the logger in your testplan. 
Testground will prefix those logs with the specific instance's sequence number and a hash id, so you will have context for which node is outputting what
logging.

## Basic example: TCP
Let's walk through an example that removes all protocol complexity, and leaves only the testground complexity: it can be found at `test-plan/movethebytes/data-transfer/rust/bin/tcp.rs`.

Let's have a test that spins up nodes that can connect over TCP, half of these nodes are going to be "server" nodes, the other half "client" nodes. When we run the test, we can run with only 2 nodes, one of which is a server, the other a client, but we will assume that we can have more than two instances.

In general, the test is separated into 3 parts
1) the main function
2) the test function
3) the "client" & "server" functions

### The main function
The job of the main function is to call the correct test function.
In the tcp case, this just calls the `run_tcp_test`.
You can technically run different tests from the same file by using
different parameters, as you can see in the `sdk-rust` example: 
`https://github.com/testground/sdk-rust/blob/master/examples/example.rs`

### The test function
This is where the testground `Client` gets built.

The `Client` is how you have access to metadata about the instance itself, such
as the `global_seq` number, which gets used in a few places, and how to
coordinate with the other instances. This coordination happens "out of band" of
whatever server/client configuration you write in the test itself. It's how you
can pass around your instance's address & get the address of the other
instances, as well as any generated Cids you need to exchange. You also use the
client to sync the timing of things like:
- ensuring instance A doesn't dial instance B until instance B has been created
  & is waiting for a connection
- ensuring instance C keeps running until instance D has been able to request
  and verify some content from instance C

The "run test" function not only creates the client, but also helps the test
runner determine what "kind" of role this instance is going to have in the
test. 

In this particular case, we will only have 2 roles, a `server` and a `client`.
In a p2p context `server` and `client` don't always make sense. For example, if
you are testing a network's ability to connect to every other node in the
network, you may only have one "role" that every node in that test takes on.
You may have more than 2 roles!

But in this case, and in the case of this data transfer plan (currently), we are
only looking at point to point transfer between two nodes.

We differentiate roles by using the global sequence number. In this case, nodes
with `seq % 2 = 0` are clients, and `seq % 2 = 1` are servers.

### The "server" and "client" functions
These functions set up their nodes based on the expected behaviour. But in
general, this is what each function needs to do:
    - get your ip address
    - build your node & connect
    - communicate your address (or other information) to the rest of the
      network
    - connect to other nodes
    - test!
    - cleanup/shutdown

#### getting your ip address:
```rust
async fn get_local_addr() -> Result<String> {
    match if_addrs::get_if_addrs()
        .unwrap()
        .into_iter()
        .find(|iface| iface.name == "eth1")
        .unwrap()
        .addr
        .ip()
    {
        std::net::IpAddr::V4(addr) => format!("{addr}:{LISTENING_PORT}"),
        std::net::IpAddr::V6(_) => unimplemented!(),
    }
}
```
Each instance has its own IP address but can use identical ports. In the above
example, we use `if_addrs` to get the instance's IP address, and return it as
a `String` that can parse to a `SocketAddr`. See another example in the 
[rust-libp2p ping test plan](https://github.com/libp2p/test-plans/blob/master/ping/rust/src/lib.rs#L40), where this method was found.

#### build and run your node
This is different for each test. In the TCP test, the we build and run a Tcp server bound to the previously found `local_addr`. In a more complex test, this is where you may build and run your node.

#### advertise your address and connect to other nodes:
To advertise your address (or any other data, like available `Cid`s), use the `client.publish` method. In the TCP example
we do:
```rust
    # create a payload
    let payload = serde_json::json!({
        "ID": seq,
        "Addrs": [local_addr],
    });

    # pubilsh it to a topic
    client.publish("server", Cow::Owned(payload)).await?;
```

To obtain those addresses, use `client.subscribe`:
```rust
    # use the test_instance_count to understand how many addresses we should be
    expecting
    let server_count = client.run_parameters().test_instance_count as usize / 2;
    # create a stream of addresses 
    let mut server_address_stream = client
        .subscribe("server", responders_count)
        .await
        .take(responders_count)
        .map(|a| {
            let value = a.unwrap();
            value["Addrs"][0].as_str().unwrap().to_string()
        });

    # iterate over that stream
    while let Some(addr) = server_address_stream.next().await {
        let addr: std::net::SocketAddr = addr.parse()?;
        # connect to `addr`
    }
    # make sure to drop the stream
    drop(server_address_stream);
```

First, we need some way to understand how many addresses we should be expecting. In this case, since we know half of the nodes are reporting their addresses, we should expect half the test count size. 

Then we create a stream of addresses from the `client.subscribe` stream. 

Next, we iterate through that stream to connect to the server nodes.

Finally, we `drop` the stream. Check out the
[subscribe](https://docs.rs/testground/0.4.0/testground/client/struct.Client.html#method.subscribe) docs for more info.

__Notes on connecting__
In the few examples I have written (tcp, quic, krakensync), I've set up nodes that _expect to be dialed_ and nodes that _do the dialing_. This side steps a whole set of dialing problems that occur when two nodes attempt to dial each other at the same time.

Another example of connecting nodes, but without separating the nodes into "dialers" and "dialees" can be found in the [libp2p test-plans ping
example](https://github.com/libp2p/test-plans/blob/master/ping/rust/src/lib.rs#L68-L73). To dedup connections and side step the "simultaneous dialing" problem, each instance should only connects to nodes that have already advertised their addresses.

#### test!
In the TCP example, we just ensure there is a connection. In the data transfer example, we will attempt to fetch a CID from the network.

#### syncing nodes using `signal` and `signal_and_wait`
In our TCP example, we need to ensure that the server nodes stay running until the client nodes have had a chance to connect. We do this by using `signal` and `signal_and_wait`.

Both methods allow you to signal on a topic, in the case of TCP that topic was "done".

The `signal` method will signal on this topic and then return. The `signal_and_wait` method will signal on this topic, wait until the `target` number of signals has been reached, and then return.

In the TCP example, we use `signal` for the client nodes and `signal_and_wait` for the server nodes.

#### record success!
If you made it to the end of your test, have the instance record a success: `client.record_success()?`

## Expected test parameters
Use the `fixtures/10MiB.car` file as your input.
The expected latency, bandwidth, etc, is notated in the `_compositions/krakensync.toml` file.

How to add network configuration, adapted from From the libp2p rust testplans:
```rust
    use testground::network_conf::{
        FilterAction, LinkShape, NetworkConfiguration, RoutingPolicyType, DEFAULT_DATA_NETWORK,
    };
    ...

    let latency: u64 = client
        .run_parameters()
        .test_instance_params
        .get("latency")
        .unwrap()
        .parse()
        .unwrap();

    let bandwidth: u64 = client
        .run_parameters()
        .test_instance_params
        .get("bandwidth")
        .unwrap()
        .parse()
        .unwrap();


        let latency = Duration::from_millis(latency)
            .as_nanos()
            .try_into()
            .unwrap();

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
            callback_state: format!("network-configured-{}", i),
            callback_target: Some(client.run_parameters().test_instance_count),
            routing_policy: RoutingPolicyType::AllowAll,
        };

        client.configure_network(network_conf).await.unwrap();
```

## metrics
Ensure the following metrics with the specified text are output to the test instance. This can be done in the test plan itself, or in the protocol library.
I ran into some trouble getting testground to pick up on the logging specified in the procotol library itself, but it was able to pick up on `println!` calls.

| Constant                 | Definition                                                                      | Unit            | Log output                    |
|--------------------------|---------------------------------------------------------------------------------|-----------------|-------------------------------|
| REQUESTER_FETCH_DURATION | the duration of time from initiation of fetch request to last byte arrival      | Milliseconds    | “REQUESTER_FETCH_DURATION=10” |
| MESSAGE_RECEIVED         | total number of messages received during the test                               | monotonic count | “MESSAGE_RECEIVED=10”         |
| MESSAGE_SENT             | total number of message sent during the test                                    | monotonic count | “MESSAGE_SENT=10”             |
| REQUESTER_TTFB           | the duration of time from initiation of fetch request to the first byte arrival | Milliseconds    | “REQUESTER_TTFB”              |

