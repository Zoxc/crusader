# CHANGELOG

The **Crusader Network Tester** measures network rates and latency
in the presence of upload and download traffic.
It produces plots of the traffic rates,
latency and packet loss.

This file lists the changes that have occurred since January 2024 in the project.

## Unreleased 

- Add `crusader remote` command to start a web server
   listening on port 35482 that has much of the functionality of
   the `crusader-gui`.
   The GUI requests the server address and various other parameters
   and displays the resulting chart in the web page.

## 0.0.12 - 2024-07-31
- Create UDP server for each server IP (fixes #22)
- Improve error handling for log messages 
- Change date format to use YYYY-MM-DD

## 0.0.11 - 2024-07-31
- Log file includes timestamps and version number
- Add peer latency measurements
- Add version to title bar of GUI
- Add `plot_max_bandwidth` and `plot_max_latency` command line options

## 0.0.10 - 2024-01-09
- Specify plot title
- Ignore ENOBUFS error 
