use bytes::{Bytes, BytesMut};
use futures::{Sink, Stream};
use std::error::Error;
use tokio::net::{self};
use tokio_util::codec::Framed;

use crate::protocol::{codec, receive, send, Hello};

async fn hello<S: Sink<Bytes> + Stream<Item = Result<BytesMut, S::Error>> + Unpin>(
    stream: &mut S,
) -> Result<(), Box<dyn Error>>
where
    S::Error: Error + 'static,
{
    let hello = Hello::new();

    send(stream, &hello).await?;
    let server_hello: Hello = receive(stream).await?;

    if hello != server_hello {
        panic!(
            "Mismatched server hello, got {:?}, expected {:?}",
            server_hello, hello
        );
    }

    Ok(())
}

async fn test_async(server: &str) -> Result<(), Box<dyn Error>> {
    let control = net::TcpStream::connect((server, 30481)).await?;

    let server = control.peer_addr()?;

    println!("Connected to server {}", server);

    let mut control = Framed::new(control, codec());

    hello(&mut control).await?;

    Ok(())
}

pub fn test(host: &str) {
    // Create the runtime
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Spawn the root task
    rt.block_on(test_async(host)).unwrap();
}
