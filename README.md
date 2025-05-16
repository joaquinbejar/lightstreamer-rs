# Lightstreamer Rust Client 

This project is a partial implementation of the Lightstreamer TLCP (Text-based Live Connections Protocol) in Rust. It provides a client SDK to interact with Lightstreamer servers, focused on supporting the specific needs of the [ig_trading_api](https://github.com/joaquinbejar/ig_trading_api) project.

## Features

- Full-duplex WebSocket-based connection mode.
- Subscriptions to items and item groups.
- MERGE subscription mode.
- Listening to connection events and messages.
- Configuration of connection options and connection details.
- Subscription lifecycle management.
- Retrieval of real-time item updates.

Please note that this SDK currently does not support all the features and capabilities of the full Lightstreamer protocol. It has been developed to cover the requirements of the ig_trading_api project mentioned above. Features like other connection modes, subscription modes (DISTINCT, RAW, COMMAND), and some other advanced options are not implemented at this time.

## Installation

To use this SDK in your Rust project, add the following dependency to your `Cargo.toml`:

```toml
[dependencies]
lightstreamer-rs = "0.1.0"
```

## Usage

Here's a minimal example of how to use the Lightstreamer Rust Client SDK:

```rust
use lightstreamer_client::ls_client::LightstreamerClient;
use lightstreamer_client::subscription::{Subscription, SubscriptionMode};

#[tokio::main]
async fn main() {
    // Create a Lightstreamer client
    let client = LightstreamerClient::new(
        Some("http://push.lightstreamer.com/lightstreamer"), // Lightstreamer server
        Some("DEMO"), // adapter set
        None, // username
        None, // password
    ).unwrap();

    // Create a subscription
    let mut subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec!["item1".to_string(), "item2".to_string()]),
        Some(vec!["field1".to_string(), "field2".to_string()]),
    ).unwrap();

    // Subscribe and connect
    client.subscribe(subscription);
    client.connect(None).await.unwrap();
}
```

For a more advanced example of how to use the SDK to subscribe to item updates, refer to the main.rs file in the project source code. It demonstrates creating a Lightstreamer client, setting up subscriptions, handling item updates, and managing the connection lifecycle with a configurable number of connection attempts.

For more details on using the SDK, please refer to the reference documentation.

## Documentation

The full SDK documentation is available at [docs.rs](https://docs.rs/lightstreamer-rs).

## Project Structure

Although this SDK does not provide a complete implementation of the Lightstreamer protocol, it has been built with a solid structure and scaffolding, similar to the official Lightstreamer libraries. The code is documented, and the project is designed to facilitate contributions from the community to add support for missing features.

## License

This project is licensed under the GPL-3.0 License. See the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome. Please open an issue or submit a pull request to propose changes and help complete the SDK with additional Lightstreamer features.