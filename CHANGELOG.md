# CHANGELOG

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It produces plots of the traffic rates,
latency and packet loss.

This file lists the changes that have occurred since January 2024 in the project.

## Unreleased

* Show throughput, latency, and packet loss summaries in plots and with the `test` command
* Rename both option to bidirectional
* Rename `--latency-peer-server` to `--latency-peer-address`
* Continuous clock synchronization with the latency monitor
* Support opening result files in the GUI by drag and drop
* Add `--out-name` command line option to specify result filename prefix
* Change filename prefix for both raw result and plots to `test`
* Add file dialog to save options in GUI
* Add buttons to save and load from the `crusader-results` folder in GUI
* Add an `export` command line command to convert result files to JSON
* Change timeout when connecting a peer to the server to 8 seconds
* Hide advanced parameters in GUI
* Add a reset parameters button in GUI
* Add an option to measure latency-only for the client in the GUI
* Don't allow peers to connect with the regular server
* Added average lines in GUI

## 0.2 - 2024-08-29

* Added support for local discovery of server and peers using UDP port 35483
* The `test` command line option `--latency-peer` is renamed to `--latency-peer-server`.
  A new flag `--latency-peer` will instead search for a local peer.
* Improved error messages
* Fix date/time display in remote web page
* Rename the `Latency` tab to `Monitor`
* Change default streams from 16 to 8.
* Change default throughput sample interval from 20 ms to 60 ms.
* Change default load duration from 5 s to 10 s.
* Change default grace duration from 5 s to 10 s.
* Fix serving from link-local interfaces on Linux
* Fix peers on link-local interfaces
* Show download and upload plots for aggregate tests in the GUI
* Added a shortcut (space) to stop the latency monitor
* Change timeout when connecting to servers and peers to 8 seconds
* Added average lines to the plot output
* Show interface IPs when starting servers

## 0.1 - 2024-08-21

* Added `crusader remote` command to start a web server listening on port 35482.
   It allows starting tests on a separate machine and
   displays the resulting charts in the web page.
* Use system fonts in GUI
* Improved error handling and error messages
* Added `--idle` option to the client to test without traffic
* Save results in a `crusader-results` folder
* Allow building of a server-only binary
* Generated files will use a YYYY-MM-DD HH.MM.SS format
* Rename bandwidth to throughput
* Rename sample rate to sample interval
* Rename `Both` to `Aggregate` and `Total` to `Round-trip` in plots

## 0.0.12 - 2024-07-31

* Create UDP server for each server IP (fixes #22)
* Improved error handling for log messages
* Changed date format to use YYYY-MM-DD in logs

## 0.0.11 - 2024-07-29

* Log file includes timestamps and version number
* Added peer latency measurements
* Added version to title bar of GUI
* Added `plot_max_bandwidth` and `plot_max_latency` command line options

## 0.0.10 - 2024-01-09

* Specify plot title
* Ignore ENOBUFS error
