_First cut at numerical summary window documentation_
-------
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

Crusader smooths all the latency samples over a 400 msec window. The values shown below display the maximum of those smoothed values. This emphasizes the peaks of latency.

* Download - Summarizes the round-trip latency during the Download portion of the test.
  Also displays the measured one-way delay for Download (from server to client)
  and Upload (client to server)
* Upload - Summarizes the latency for the Upload portion of the test
* Bidirectional - Summarizes the latency or the Bidirectional portion of the test
* Idle latency - Measured latency when no traffic is present.
* Latency sample interval - Interval between latency measurements


### Packet loss

<img width="184" alt="image" src="https://github.com/user-attachments/assets/6b3681b3-c369-48ae-8b49-6e9dec034b82">

* Download - Summarizes packet loss during the Download portion of the test
* Upload - Summarizes packet loss during the Upload portion of the test
* Bidirectional - Summarizes packet loss during the Bidirectional portion of the test 
