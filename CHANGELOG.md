# Changelog

The **Crusader Network Tester** measures network rates and latency
while downloading and uploading data.
It produces plots of the traffic rates,
latency and packet loss.

This file lists the changes that have occurred since January 2024 in the project:

## Unreleased 

- _info about changes that have not been added to a release._

## 0.0.12 - 2024-07-31
- Create UDP server for each server IP
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
- Fix clippy warnings
