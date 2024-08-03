# Crusader Network Tester

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It produces plots of the traffic rates,
latency and packet loss.
Crusader only uses TCP and UDP ports 35481 for its tests.

**Pre-built binaries** for Windows, Mac, Linux, 
and Android are available on the
[Releases](https://github.com/Zoxc/crusader/releases) page.

**Status:** The latest Crusader version is 0.0.12.
   See the [CHANGELOG.md](./CHANGELOG.md) file for details.

## Crusader GUI

A test run requires two separate computers,
both running Crusader:
a **server** that listens for connections, and
a **client** that initiates the test.

The Crusder GUI incorporates both the server and
the client, and allows you to interact with results.
To use it, download the proper binary from the 
[Releases](https://github.com/Zoxc/crusader/releases) page
then open it.

You will see the following window.
Enter the address of another computer that's 
running the Crusader server, then click **Start test**. 
When the test is complete, the **Result** tab shows a
chart similar like the second image below.
(An easy way to run the server is to start the Crusader GUI
on another computer, then choose the **Server** tab.)

<img src="media/gui-client.png">

<img src="media/gui.png">

## Understanding the results

The Crusader client creates three bursts of traffic
(by default, five seconds each):
download only, upload only, then bi-directional traffic.
Each burst is separated by several seconds of idle time.

The Crusader GUI displays the results of the test with
three plots (see image above):

* The **Bandwidth** plot (top) shows the bursts of traffic:
green is download, blue is upload, and
the purple line is the instantaneous
sum of the download plus upload.

* The **Latency** plot (middle) shows the corresponding latency.
Blue is the (uni-directional) time from the client to the server.
Green shows the time from the server to the client (one direction).
Black shows the sum from the client to the server 
and back (bi-directional).

* The **Bandwidth** plot (bottom) has green and blue marks
that mark times when packets were lost.

<!-- <img src="media/plot.png"> -->

## Building Crusader from source

The commands below build all three binaries.
Executables are placed in `src/target/release`

```sh
cd src
cargo build --release
```

## Running Crusader from the command line

See also the [command-line options](#command-line-options) below.

### GUI Program
This command starts the GUI program.

```sh
cd src/target/release
./crusader-gui 
```

### Crusader Server

To host a Crusader server, on the _server machine,_ run:

```sh
cd src/target/release
./crusader serve
```

### Crusader Client
To start a test run, on the _client machine,_ run:

```sh
cd src/target/release
./crusader test <server-ip>
```

## Command-line options

**Usage: crusader \<COMMAND>**

**Commands:**

- **serve**   Runs the server
- **test**    Runs a test client against a specified server and saves the result to the current directory. By default this does a download test, an upload test, and a test doing both download and upload while measuring the latency to the server
- **plot**    Plots a previous result
- **remote**  Allows the client to be controlled over a web server
- **help**    Print this message or the help of the given subcommand(s)

**Options:**

- **-h, --help**     Print help
- **-V, --version**  Print version
- **--plot\_max\_bandwidth** 
- **--plot\_max\_latency**
- _Other options?_

## Troubleshooting

- Crusader requires that TCP and UDP ports 35481 for its tests.
   Check that your firewall is letting those ports through.
