# Understanding Crusader Results

The Crusader GUI provides a compact summary of the test data.
Here are some hints for evaluating the results.

## Result window

![Result with statistics](./media/Crusader-Result-with-stats.png)

As described in the [README](../README.md), Crusader tests the connection
using three bursts of traffic.
The Throughput, Latency, and Packet loss are shown in the charts.
In the image above notice:

* Hovering over a chart shows crosshairs that give the throughput
  or latency of that point in the chart.
  In the screen shot above, the download and upload latency
  are about 160 msec.
* Hovering over, or clicking the ⓘ symbol opens a window that gives
  a summary of the statistics.
  See the description below for more definitions of the values.
* Clicking a legend ("color") in the charts shows/hides that chart.
  In the screen shot above, the Latency's "Down" legend has been clicked,
  hiding the Down chart, and showing only the Up and Round-trip values.
* Crusader shows that latency increases dramatically both
  for the download and upload portions of the test.
  During download and upload, it's about 160 msec;
  during the bidirectional portion of the test, it peaks at over 600 msec.

## Numerical Summary Windows

The Crusader GUI displays charts showing Throughput, Latency, and Packet loss. The ⓘ symbol opens a window showing a numerical summary of the charted data.

### Throughput

<img width="239" alt="image" src="https://github.com/user-attachments/assets/f2078aff-17e1-4599-8e10-c798a917f040">

* Download - Average throughput (total data received divided by the elapsed time) during the Download portion of the test
* Upload - Average throughput during the Upload portion of the test
* Bidirectional - Sum of the Download and Upload throughputs during the Bidirectional portion of the test. Also displays the Download and Upload throughputs.
* Streams - number of TCP connections used in each direction
* Stream Stagger - The delay between the start of each stream  
* Throughput sample interval - Interval between throughput measurements

### Latency

<img width="209" alt="image" src="https://github.com/user-attachments/assets/fbcc361f-15cf-45d0-9f40-a497c12b0cfd">

Crusader smooths all the latency samples over a 400 msec window.
The values shown in the window display the maximum of those smoothed values.
This emphasizes the peaks of latency.

* Download - Summarizes the round-trip latency during the Download portion of the test.
  Also displays the measured one-way delay for Download (from server to client)
  and Upload (client to server)
* Upload - Summarizes the latency for the Upload portion of the test
* Bidirectional - Summarizes the latency for the Bidirectional portion of the test
* Idle latency - Measured latency when no traffic is present.
* Latency sample interval - Interval between latency measurements


### Packet loss

<img width="184" alt="image" src="https://github.com/user-attachments/assets/6b3681b3-c369-48ae-8b49-6e9dec034b82">

* Download - Summarizes packet loss during the Download portion of the test
* Upload - Summarizes packet loss during the Upload portion of the test
* Bidirectional - Summarizes packet loss during the Bidirectional portion of the test 
