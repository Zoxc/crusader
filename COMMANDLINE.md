# Running Crusader from the command line

## Server

To host a Crusader server, run this on the _server_ machine:

```sh
crusader serve
```

## Client

To start a test, run this on the _client machine_.
See the [command-line options](.options-for-the-test-command) for details.

```sh
crusader test <server-ip>
```

## Remote

To host a web server that provides remote control of a Crusader client,
run the command below, then connect to
`http://ip-of-the-crusader-device:35482`

```sh
crusader remote
```

## Plot

Crusader creates a `.png` file from a `.crr` file using `./crusader plot path-to-crr-file`
The resulting `.png` is saved in the same directory as the input file.

## Export

Crusader exports raw data samples from a `.crr` file
into a `.json` file using `./crusader export path-to-crr-file`
The resulting `.json` is saved in the same directory as the input file.

## Options for the `test` command

**Usage: crusader test [OPTIONS] \<SERVER>**

**Arguments:** \<SERVER>

**Options:**

* **`--download`**
          Run a download test
* **`--upload`**
          Run an upload test
* **`--bidirectional`**
          Run a test doing both download and upload
* **`--idle`**
          Run a test only measuring latency. The duration is specified by `grace_duration`
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
* **`--latency-peer-address <LATENCY_PEER_ADDRESS>`**
          Specifies another server (peer) which will
          also measure the latency to the server independently of the client
* **`--latency-peer`**
          Use another server (peer) which will also measure the latency to the server independently of the client
* **`--out-name <OUT_NAME>`**
          The filename prefix used for the test result raw data and plot filenames
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

## Building

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
