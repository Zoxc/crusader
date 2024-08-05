# Crusader Network Tester

![Crusader Results Screenshot](./media/Crusader-Results.png)

[![GitHub Release](https://img.shields.io/github/v/release/Zoxc/crusader)](https://github.com/Zoxc/crusader/releases)

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It also incorporates a continuous latency tester for
monitoring background responsiveness.
It produces plots of the traffic rates,
latency and packet loss.
Crusader only uses TCP and UDP ports 35481 for its tests.

**Pre-built binaries** for Windows, Mac, Linux,
and Android are available on the
[Releases](https://github.com/Zoxc/crusader/releases) page.

**Status:** The latest Crusader release is shown above.
  See the [CHANGELOG.md](./CHANGELOG.md)
  file for details.

## Crusader GUI

A test run requires two separate computers,
both running Crusader:
a **server** that listens for connections, and
a **client** that initiates the test.

The Crusader GUI incorporates both the server and
the client and allows you to interact with results.
To use it, download the proper binary from the
[Releases](https://github.com/Zoxc/crusader/releases) page
then open it.

The window below opens.
Enter the address of another computer that's
running the Crusader server, then click **Start test**.
When the test is complete, the **Result** tab shows a
chart like the second image below.
(An easy way to run the server is to start the Crusader GUI
on another computer, then choose the **Server** tab.)

![Crusader Client Screenshot](./media/Crusader-Client.png)

![Crusader Results Screenshot](./media/Crusader-Results.png)

## Understanding the results

A Crusader test creates three bursts of traffic.
By default, it generates five seconds each of
download only, upload only, then bi-directional traffic.
Each burst is separated by several seconds of idle time.

The Crusader GUI displays the results of the test with
three plots (see image above):

* The **Throughput** plot shows the bursts of traffic:
green is download (from server to client),
blue is upload, and
the purple line is the instantaneous
sum of the download plus upload.

* The **Latency** plot shows the corresponding latency.
Blue is the (uni-directional) time from the client to the server.
Green shows the time from the server to the client (one direction).
Black shows the sum from the client to the server
and back (bi-directional).

* The **Packet Loss** plot has green and blue marks
that indicate times when packets were lost.

See also the [GUI options](#gui-options) section.

## Building Crusader from source

Use [pre-built binaries](https://github.com/Zoxc/crusader/releases)
for everyday tests.
To develop or debug Crusader, use the commands below
to build all three binaries.
Executables are placed in `src/target/release`

```sh
cd src
cargo build --release
```

## Running Crusader from the command line

See also the
[command-line options](#command-line-options) section.

### GUI Program

This command starts the GUI program.

```sh
cd src/target/release
./crusader-gui
```

### Crusader Server

To host a Crusader server, run this on the _server machine:

```sh
cd src/target/release
./crusader serve
```

### Crusader Client

To start a test, run this on the _client machine_:

```sh
cd src/target/release
./crusader test <server-ip>
```

## GUI Options

* **Client tab**
  Run the Crusader Client program
  * **Download, Upload, Both**
     checkboxes control which tests to run
  * **Streams**
  * **Load duration**
  * **Grace duration**
  * **Stream stagger**
  * **Latency sample rate**
  * **Bandwidth sample rate**
  * **Latency peer**

* **Server tab**
  Run the Crusader server to listen for other clients

* **Latency tab**
  Continually test the latency to the selected
  Crusader server until stopped.

* **Result tab**
  Display the result of the most recent client run

## Command-line options

**Usage: crusader \<COMMAND>**

**Commands:**

* **serve**  Run the server
* **test _server-IP_**   Run the client to the specified server.
  Save the result files in the current directory.
* **plot _path-to-file_**   Plot a previous result `.crr` file
* **remote**  Allow the client to be controlled over a web server
* **help**   Print this message or the help of the given subcommand(s)

**Options:**

* **-h, --help**    Print help
* **-V, --version**  Print version
* **--plot\_max\_bandwidth**
* **--plot\_max\_latency**

## Troubleshooting

* Crusader requires that TCP and UDP ports 35481 are open for its tests.
  Check that your firewall is letting those ports through.

* Create a debug build by using `cargo build`
  (instead of `cargo build --release`).
  Binaries are saved in the _src/target/debug_ directory

* To get the git commit hash of the current checkout,
  use `git rev-parse --short master`
