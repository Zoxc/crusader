# Crusader Network Tester

The Crusader Network Tester measures network rates and latency
while downloading and uploading data.
It produces plots of the traffic rates,
latency and packet loss.

A test run requires two separate computers,
both running Crusader:
a **server** that listens for connections, and
a **client** that initiates the test.

Client and server talk (only) on TCP and UDP port 35481.

## Setup

To build all binaries, use the commands below. Executables are placed in `src/target/release`

```sh
cd src
cargo build --release
```

## Crusader Server

To host a Crusader server, on the _server machine,_ run:

```sh
cd src/target/release
./crusader serve
```

## Crusader Client
To start a test run, on the _client machine,_ run:

```sh
cd src/target/release
./crusader test <server-ip>
```


The Crusader client creates three bursts of traffic
(by default, five seconds each):
download only, upload only, then bi-directional traffic.
Each burst is separated by several seconds of idle time.

**Output:** The client produces a `.png` file showing charts (below)
as well as a `.crr` file with the raw data.

* The top plot shows the bursts of traffic:
green is download, blue is upload, and
the purple line is the instantaneous
sum of the download plus upload.

* The next plot shows the corresponding latency.
Blue is the (uni-directional) time from the client to the server.
Green shows the time from the server to the client (one direction).
Black shows the sum from the client to the server 
and back (bi-directional).

* The bottom plot has green and blue marks that show
places where packet loss occurred.

<img src="media/plot.png">

## Graphical interface

The build process also creates a binary with a
graphical interface that incorporates the server,
the client, and allows you to interact with results.
To use it, run the following command.
Enter the command below and click
**Start test** in the resulting window.

```sh
cd src/target/release
./crusader-gui 
```

<img src="media/gui-client.png">

Click the **Result** tab to see a chart similar to the one above.

<img src="media/gui.png">
