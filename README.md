# Crusader Network Tester

[![GitHub Release](https://img.shields.io/github/v/release/Zoxc/crusader)](https://github.com/Zoxc/crusader/releases)
[![Docker Hub](https://img.shields.io/badge/container-dockerhub-blue)](https://hub.docker.com/r/zoxc/crusader)
[![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Zoxc/crusader/blob/master/LICENSE-MIT)
[![Apache](https://img.shields.io/badge/license-Apache-blue.svg)](https://github.com/Zoxc/crusader/blob/master/LICENSE-APACHE)

![Crusader Results Screenshot](./media/Crusader-Result.png)

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It also incorporates a continuous latency tester for
monitoring background responsiveness.
It produces plots of the traffic rates, latency and packet loss.
Crusader uses TCP and UDP ports 35481 (only) for its tests.
The remote web server option uses TCP port 35482.
Local server discovery uses UDP port 35483.

**Pre-built binaries** for Windows, Mac, Linux,
and Android are available on the
[Releases](https://github.com/Zoxc/crusader/releases) page.
The GUI is not prebuilt for Linux and must be built from source.
A **Docker container** for running the server may be found on
[dockerhub](https://hub.docker.com/r/zoxc/crusader).

**Status:** The latest Crusader release version is shown above.
  The pre-built binaries always provide the latest version.
  See the [CHANGELOG.md](./CHANGELOG.md) file for details.

## Crusader GUI

A test run requires two separate computers,
both running Crusader:
a **server** that listens for connections, and
a **client** that initiates the test.

The Crusader GUI incorporates both the server and
the client and allows you to interact with results.
To use it, download the proper binary from the
[Releases](https://github.com/Zoxc/crusader/releases) page.

When you open the `crusader-gui` you see this window.
Enter the address of another computer that's
running the Crusader server, then click **Start test**.
When the test is complete, the **Result** tab shows a
chart like the second image below.
(An easy way to run the server is to download a copy
of the Crusader GUI
to another computer, start it, then choose the **Server** tab.)

![Crusader Client Screenshot](./media/Crusader-Client.png)

The Crusader GUI has five tabs:

* **Client tab**
  Runs the Crusader client program.
  The options shown above are described in the
  [Command-line options](./#command-line-options) section.

* **Server tab**
  Runs the Crusader server, listening for connections from other clients

* **Remote tab**
  Starts a webserver (default port 35482).
  A browser that connects to that port can initiate
  a test to a Crusader server.
  
* **Monitor tab**
  Continually displays the latency to the selected
  Crusader server until stopped.

* **Result tab**
  Displays the result of the most recent client run

## The Result Tab

![Crusader Results Screenshot](./media/Crusader-Result.png)

A Crusader test creates three bursts of traffic.
By default, it generates ten seconds each of
download only, upload only, then bi-directional traffic.
Each burst is separated by several seconds of idle time.

The Crusader Result tab displays the results of the test with
three plots (see image above):

* The **Throughput** plot shows the bursts of traffic.
Green is download (from server to client),
blue is upload, and
the purple line is the instantaneous
sum of the download plus upload.

* The **Latency** plot shows the corresponding latency.
Green shows the  (uni-directional) time from the server to the client.
Blue is the (uni-directional) time from the client to the server.
Black shows the sum from the client to the server
and back (round-trip time).

* The **Packet Loss** plot has green and blue marks
that indicate times when packets were lost.

## Running Crusader from the command line

### Server

To host a Crusader server, run this on the _server_ machine:

```sh
crusader serve
```

### Client

To start a test, run this on the _client machine_:

```sh
crusader test <server-ip>
```

### Remote

To host a web server that provides remote control of a Crusader client,
run the command below, then connect to
`http://ip-of-the-crusader-device:35482`

```sh
crusader remote
```

### Options for the `test` command

**Usage: crusader test [OPTIONS] \<SERVER>**

**Arguments:** \<SERVER>

**Options:**

* **`--download`**
          Run a download test
* **`--upload`**
          Run an upload test
* **`--both`**
          Run a test doing both download and upload
* **`--port <PORT>`**
          Specifies the TCP and UDP port used by the server
          [default: 35481]
* **`--streams <STREAMS>`**
          The number of TCP connections used to generate
           traffic in a single direction
          [default: 16]
* **`--stream-stagger <SECONDS>`**
          The delay between the start of each stream
          [default: 0.0]
* **`--load-duration <SECONDS>`**
          The duration in which traffic is generated
          [default: 5.0]
* **`--grace-duration <SECONDS>`**
          The idle time between each test
          [default: 1.0]
* **`--latency-sample-interval <MILLISECONDS>`**
          [default: 5.0]
* **`--throughput-sample-interval <MILLISECONDS>`**
          [default: 20.0]
* **`--plot-transferred`**
          Plot transferred bytes
* **`--plot-split-throughput`**
          Plot upload and download separately and plot streams
* **`--plot-max-throughput <BPS>`**
          Sets the axis for throughput to at least this value.
          SI units are supported so `100M` would specify 100 Mbps
* **`--plot-max-latency <MILLISECONDS>`**
          Sets the axis for latency to at least this value
* **`--plot-width <PIXELS>`**
* **`--plot-height <PIXELS>`**
* **`--plot-title <PLOT_TITLE>`**
* **`--latency-peer-server <LATENCY_PEER_SERVER>`**
          Specifies another server (peer) which will
          also measure the latency to the server independently of the client
* **`--latency-peer`**
          Use another server (peer) which will also measure the latency to the server independently of the client
* **`-h, --help`**
          Print help (see a summary with '-h')

## Building Crusader from source

Use [pre-built binaries](https://github.com/Zoxc/crusader/releases)
for everyday tests if available.

To develop or debug Crusader, use the commands below
to build the full set of binaries.
Executables will be placed in `src/target/release`

### Required dependencies

* A Rust and C toolchain.

### Additional dependencies for `crusader-gui`

Development packages for:

* `fontconfig`

To install these on Ubuntu:

```sh
sudo apt install libfontconfig1-dev
```

### Building

To build the `crusader` command line executable:

```sh
cd src
cargo build -p crusader --release
```

To build both command line and GUI executables:

```sh
cd src
cargo build --release
```

To build a docker container running the server:

```sh
cd docker
docker build .. -t crusader -f server-static.Dockerfile
```

## Troubleshooting

* On macOS, the first time you double-click
  the pre-built `crusader` or `crusader-gui` icon,
  the OS requires you to use **System Preferences -> Security**
  to permit Crusader to run.
  
* Crusader requires that TCP and UDP ports 35481 are open for its tests.
  Crusader also uses ports 35482 for the remote webserver
  and port 35483 for discovering other Crusader Servers.
  Check that your firewall is letting those ports through.

* The [Releases](https://github.com/Zoxc/crusader/releases) page
  has pre-built binaries.
  You can build your own using the instructions above.
  
* Create a debug build by using `cargo build`
  (instead of `cargo build --release`).
  Binaries are saved in the _src/target/debug_ directory

* Current binaries display the full commit hash in the log files.
  To get the git commit hash of the current checkout,
  use `git rev-parse HEAD`.
  
* The message `Warning: Load termination timed out. There may be residual untracked traffic in the background.` is not harmful. It may happen due to the TCP termination being lost
  or TCP incompatibilities between OSes.
  It's likely benign if you see throughput and latency drop
  to idle values after the tests in the graph.

* The up and down latency measurement rely on symmetric stable latency
measurements to the server.
These values may be wrong if those assumption don't hold on test startup.

* The up and down latency measurement may slowly get out of sync due to
clock drift. Clocks are currently only synchronized on test startup.
