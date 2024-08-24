# CHANGELOG

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It produces plots of the traffic rates,
latency and packet loss.

This file lists the changes that have occurred since January 2024 in the project.

## Unreleased

* Added support for local discovery of servers using UDP port 35483
* Improved error messages
* Fix date/time display in remote web page
* Rename the `Latency` tab to `Monitor`

## 0.1 - 2024-08-21

* Added `crusader remote` command to start a web server listening on port 35482.
   It allows starting tests on a separate machine and displays the resulting charts in the web page.
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
