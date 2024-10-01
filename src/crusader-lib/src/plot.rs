use anyhow::{anyhow, Context};
use image::{ImageBuffer, ImageFormat, Rgb};
use plotters::coord::types::RangedCoordf64;
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use plotters::style::{register_font, RGBColor};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;
use std::{cmp, fmt::Write, mem};

use crate::file_format::{RawPing, RawResult, TestData, TestKind};
use crate::protocol::RawLatency;
use crate::test::{unique, PlotConfig};

const UP_COLOR: RGBColor = RGBColor(37, 83, 169);
const DOWN_COLOR: RGBColor = RGBColor(95, 145, 62);

fn darken(color: RGBColor, d: f64) -> RGBColor {
    RGBColor(
        (color.0 as f64 * d).round() as u8,
        (color.1 as f64 * d).round() as u8,
        (color.2 as f64 * d).round() as u8,
    )
}

pub fn register_fonts() {
    register_font(
        "sans-serif",
        FontStyle::Normal,
        include_bytes!("../Ubuntu-Light.ttf"),
    )
    .map_err(|_| ())
    .unwrap();
}

impl RawResult {
    pub fn to_test_result(&self) -> TestResult {
        let throughput_interval = self.config.bandwidth_interval;

        let stream_groups: Vec<_> = self
            .stream_groups
            .iter()
            .map(|group| TestStreamGroup {
                download: group.download,
                both: group.both,
                streams: (0..(group.streams.len()))
                    .map(|i| {
                        let bytes: Vec<_> = (0..=i)
                            .map(|i| to_float(&group.streams[i].to_vec()))
                            .collect();
                        let bytes: Vec<_> = bytes.iter().map(|stream| stream.as_slice()).collect();
                        TestStream {
                            data: sum_bytes(&bytes, throughput_interval),
                        }
                    })
                    .collect(),
            })
            .collect();

        let process_bytes = |bytes: Vec<Vec<(u64, u64)>>| -> Vec<(u64, f64)> {
            let bytes: Vec<_> = bytes.iter().map(|stream| to_float(stream)).collect();
            let bytes: Vec<_> = bytes.iter().map(|stream| stream.as_slice()).collect();
            sum_bytes(&bytes, throughput_interval)
        };

        let groups: Vec<_> = self
            .stream_groups
            .iter()
            .map(|group| {
                let streams: Vec<_> = group.streams.iter().map(|stream| stream.to_vec()).collect();
                let single = process_bytes(streams);
                (group, single)
            })
            .collect();

        let find = |download, both| {
            groups
                .iter()
                .find(|group| group.0.download == download && group.0.both == both)
                .map(|group| group.1.clone())
        };

        let download_bytes_sum = find(true, false);
        let both_download_bytes_sum = find(true, true);

        let combined_download_bytes: Vec<_> = [
            download_bytes_sum.as_deref(),
            both_download_bytes_sum.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let combined_download_bytes = sum_bytes(&combined_download_bytes, throughput_interval);

        let upload_bytes_sum = find(false, false);

        let both_upload_bytes_sum = find(false, true);

        let combined_upload_bytes: Vec<_> = [
            upload_bytes_sum.as_deref(),
            both_upload_bytes_sum.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let combined_upload_bytes = sum_bytes(&combined_upload_bytes, throughput_interval);

        let both_bytes = self.both().then(|| {
            sum_bytes(
                &[
                    both_download_bytes_sum.as_deref().unwrap(),
                    both_upload_bytes_sum.as_deref().unwrap(),
                ],
                throughput_interval,
            )
        });

        let pings = self.pings.clone();

        let mut throughputs = HashMap::new();

        let mut add_throughput =
            |stream: &Option<Vec<(u64, f64)>>, kind: TestKind, sub: TestKind| {
                if let Some(stream) = stream {
                    if let Some(t) = throughput(
                        stream,
                        self.test_data.iter().find(|d| d.kind == kind),
                        self.config.load_duration,
                    ) {
                        throughputs.insert((kind, sub), t);
                    }
                }
            };

        add_throughput(&download_bytes_sum, TestKind::Download, TestKind::Download);
        add_throughput(&upload_bytes_sum, TestKind::Upload, TestKind::Upload);
        add_throughput(
            &both_download_bytes_sum,
            TestKind::Bidirectional,
            TestKind::Download,
        );
        add_throughput(
            &both_upload_bytes_sum,
            TestKind::Bidirectional,
            TestKind::Upload,
        );
        add_throughput(
            &both_bytes,
            TestKind::Bidirectional,
            TestKind::Bidirectional,
        );

        let add_latency = |map: &mut HashMap<Option<TestKind>, LatencySummary>,
                           loss: &mut HashMap<Option<TestKind>, (f64, f64)>,
                           stream: &Option<Vec<(u64, f64)>>,
                           kind: TestKind,
                           smooth_pings: &[RawPing],
                           pings: &[RawPing]| {
            if let Some(stream) = stream {
                if let Some(t) = ping_peak(
                    stream,
                    self.test_data.iter().find(|d| d.kind == kind),
                    self.config.load_duration,
                    smooth_pings,
                ) {
                    map.insert(Some(kind), t);
                }
                if let Some(t) = ping_loss(
                    stream,
                    self.test_data.iter().find(|d| d.kind == kind),
                    self.config.load_duration,
                    pings,
                ) {
                    loss.insert(Some(kind), t);
                }
            }
        };

        let latency_map = |pings: &[RawPing]| {
            let mut latencies = HashMap::new();
            let mut loss = HashMap::new();

            let smooth_pings = smooth_ping(
                pings,
                (self.config.ping_interval * 3).max(Duration::from_millis(200)),
            );

            add_latency(
                &mut latencies,
                &mut loss,
                &download_bytes_sum,
                TestKind::Download,
                &smooth_pings,
                pings,
            );
            add_latency(
                &mut latencies,
                &mut loss,
                &upload_bytes_sum,
                TestKind::Upload,
                &smooth_pings,
                pings,
            );
            add_latency(
                &mut latencies,
                &mut loss,
                &both_bytes,
                TestKind::Bidirectional,
                &smooth_pings,
                pings,
            );

            if self.idle() {
                let whole_data = TestData {
                    kind: TestKind::Bidirectional,
                    start: self.start,
                    end: self.start + self.duration,
                };

                if let Some(t) = ping_peak(&[], Some(&whole_data), self.duration, &smooth_pings) {
                    latencies.insert(None, t);
                }

                if let Some(t) = ping_loss(&[], Some(&whole_data), self.duration, pings) {
                    loss.insert(None, t);
                }
            }

            LatencyLossSummary { latencies, loss }
        };

        let latencies = latency_map(&pings);
        let peer_latencies = self
            .peer_pings
            .as_ref()
            .map(|peer_pings| latency_map(peer_pings))
            .unwrap_or_default();

        TestResult {
            raw_result: self.clone(),
            start: self.start,
            duration: self.duration,
            pings,
            both_bytes,
            both_download_bytes: both_download_bytes_sum,
            both_upload_bytes: both_upload_bytes_sum,
            download_bytes: download_bytes_sum,
            upload_bytes: upload_bytes_sum,
            combined_download_bytes,
            combined_upload_bytes,
            stream_groups,
            throughputs,
            latencies,
            peer_latencies,
        }
    }
}

pub struct TestStream {
    pub data: Vec<(u64, f64)>,
}

pub struct TestStreamGroup {
    pub download: bool,
    pub both: bool,
    pub streams: Vec<TestStream>,
}

#[derive(Debug)]
pub struct LatencySummary {
    pub total: Duration,
    pub down: Duration,
    pub up: Duration,
}

#[derive(Default)]
pub struct LatencyLossSummary {
    pub latencies: HashMap<Option<TestKind>, LatencySummary>,
    pub loss: HashMap<Option<TestKind>, (f64, f64)>,
}

pub struct TestResult {
    pub raw_result: RawResult,
    pub start: Duration,
    pub duration: Duration,
    pub download_bytes: Option<Vec<(u64, f64)>>,
    pub upload_bytes: Option<Vec<(u64, f64)>>,
    pub combined_download_bytes: Vec<(u64, f64)>,
    pub combined_upload_bytes: Vec<(u64, f64)>,
    pub both_download_bytes: Option<Vec<(u64, f64)>>,
    pub both_upload_bytes: Option<Vec<(u64, f64)>>,
    pub both_bytes: Option<Vec<(u64, f64)>>,
    pub pings: Vec<RawPing>,
    pub stream_groups: Vec<TestStreamGroup>,
    pub throughputs: HashMap<(TestKind, TestKind), f64>,
    pub latencies: LatencyLossSummary,
    pub peer_latencies: LatencyLossSummary,
}

impl TestResult {
    pub fn summary(&self) -> Result<String, anyhow::Error> {
        let mut o = String::new();

        let width = 20;

        let mut kind = |kind: Option<TestKind>| -> Result<(), anyhow::Error> {
            writeln!(
                &mut o,
                "-- {} test --",
                kind.map(|kind| kind.name()).unwrap_or("Idle")
            )?;

            if let Some(kind) = kind {
                if let Some(throughput) = self.throughputs.get(&(kind, kind)) {
                    write!(
                        &mut o,
                        "{:>width$}: {:.02} Mbps",
                        "Throughput",
                        throughput,
                        width = width
                    )?;
                    if kind == TestKind::Bidirectional {
                        if let Some(down) = self
                            .throughputs
                            .get(&(TestKind::Bidirectional, TestKind::Download))
                        {
                            if let Some(up) = self
                                .throughputs
                                .get(&(TestKind::Bidirectional, TestKind::Upload))
                            {
                                write!(&mut o, " ({:.02} Mbps down, {:.02} Mbps up)", down, up)?;
                            }
                        }
                    }
                    writeln!(&mut o)?;
                }
            }

            let mut latency =
                |latencies: &LatencyLossSummary, peer: bool| -> Result<(), anyhow::Error> {
                    if let Some(latency) = latencies.latencies.get(&kind) {
                        let label = if peer { "Peer latency" } else { "Latency" };
                        writeln!(
                            &mut o,
                            "{:>width$}: {:.01} ms ({:.01} ms down, {:.01} ms up)",
                            label,
                            latency.total.as_secs_f64() * 1000.0,
                            latency.down.as_secs_f64() * 1000.0,
                            latency.up.as_secs_f64() * 1000.0,
                            width = width
                        )?;
                    }
                    if let Some(&(down, up)) = latencies.loss.get(&kind) {
                        let label = if peer {
                            "Peer packet loss"
                        } else {
                            "Packet loss"
                        };
                        if down == 0.0 && up == 0.0 {
                            writeln!(&mut o, "{:>width$}: 0%", label)?;
                        } else {
                            writeln!(
                                &mut o,
                                "{:>width$}: {:.*}% down, {:.*}% up",
                                label,
                                if down == 0.0 { 0 } else { 2 },
                                down * 100.0,
                                if up == 0.0 { 0 } else { 2 },
                                up * 100.0,
                                width = width
                            )?;
                        }
                    }

                    Ok(())
                };

            latency(&self.latencies, false)?;
            latency(&self.peer_latencies, true)?;

            writeln!(&mut o)?;

            Ok(())
        };

        if self.raw_result.download() {
            kind(Some(TestKind::Download))?;
        }

        if self.raw_result.upload() {
            kind(Some(TestKind::Upload))?;
        }

        if self.raw_result.both() {
            kind(Some(TestKind::Bidirectional))?;
        }

        if self.raw_result.idle() {
            kind(None)?;
        }

        Ok(o)
    }
}

pub fn save_graph(
    config: &PlotConfig,
    result: &TestResult,
    name: &str,
    root_path: &Path,
) -> Result<String, anyhow::Error> {
    std::fs::create_dir_all(root_path)?;
    let file = unique(name, "png");
    save_graph_to_path(&root_path.join(&file), config, result)?;
    Ok(file)
}

pub fn save_graph_to_path(
    path: &Path,
    config: &PlotConfig,
    result: &TestResult,
) -> Result<(), anyhow::Error> {
    let img = save_graph_to_mem(config, result).context("Unable to plot")?;
    img.save_with_format(&path, ImageFormat::Png)
        .context("Unable to write plot to file")
}

pub(crate) struct ThroughputPlot<'a> {
    name: &'static str,
    color: RGBColor,
    rates: Vec<(u64, f64)>,
    smooth: Vec<(u64, f64)>,
    bytes: Vec<&'a [(u64, f64)]>,
    rate: Option<f64>,
    phase: Option<TestKind>,
    dual_rates: Option<(f64, f64)>,
}

pub(crate) fn save_graph_to_mem(
    config: &PlotConfig,
    result: &TestResult,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, anyhow::Error> {
    let mut throughput = Vec::new();

    let smooth_interval = cmp::min(
        Duration::from_secs_f64(1.0),
        result.raw_result.config.grace_duration,
    );
    let interval = result.raw_result.config.bandwidth_interval;

    result.download_bytes.as_ref().map(|bytes| {
        throughput.push(ThroughputPlot {
            name: "Download",
            color: DOWN_COLOR,
            rates: to_rates(bytes),
            smooth: smooth(bytes, interval, smooth_interval),
            bytes: vec![bytes.as_slice()],
            rate: result
                .throughputs
                .get(&(TestKind::Download, TestKind::Download))
                .cloned(),
            dual_rates: None,
            phase: Some(TestKind::Download),
        });
    });

    result.upload_bytes.as_ref().map(|bytes| {
        throughput.push(ThroughputPlot {
            name: "Upload",
            color: UP_COLOR,
            rates: to_rates(bytes),
            smooth: smooth(bytes, interval, smooth_interval),
            bytes: vec![bytes.as_slice()],
            rate: result
                .throughputs
                .get(&(TestKind::Upload, TestKind::Upload))
                .cloned(),
            dual_rates: None,
            phase: Some(TestKind::Upload),
        });
    });

    result.both_download_bytes.as_ref().map(|bytes| {
        throughput.push(ThroughputPlot {
            name: "Download",
            color: DOWN_COLOR,
            rates: to_rates(bytes),
            smooth: smooth(bytes, interval, smooth_interval),
            bytes: vec![bytes.as_slice()],
            rate: None,
            dual_rates: None,
            phase: None,
        });
    });

    result.both_upload_bytes.as_ref().map(|bytes| {
        throughput.push(ThroughputPlot {
            name: "Upload",
            color: UP_COLOR,
            rates: to_rates(bytes),
            smooth: smooth(bytes, interval, smooth_interval),
            bytes: vec![bytes.as_slice()],
            rate: None,
            dual_rates: None,
            phase: None,
        });
    });

    result.both_bytes.as_ref().map(|both_bytes| {
        throughput.push(ThroughputPlot {
            name: "Aggregate",
            color: RGBColor(149, 96, 153),
            rates: to_rates(both_bytes),
            smooth: smooth(both_bytes, interval, smooth_interval),
            bytes: vec![both_bytes.as_slice()],
            rate: result
                .throughputs
                .get(&(TestKind::Bidirectional, TestKind::Bidirectional))
                .cloned(),
            dual_rates: result
                .throughputs
                .get(&(TestKind::Bidirectional, TestKind::Download))
                .cloned()
                .and_then(|down| {
                    result
                        .throughputs
                        .get(&(TestKind::Bidirectional, TestKind::Upload))
                        .cloned()
                        .map(|up| (down, up))
                }),
            phase: Some(TestKind::Bidirectional),
        });
    });

    graph(
        config,
        result,
        &result.pings,
        &throughput,
        result.start.as_secs_f64(),
        result.duration.as_secs_f64(),
    )
}

pub fn float_max(iter: impl Iterator<Item = f64>) -> f64 {
    let mut max = iter.fold(f64::NAN, f64::max);

    if max.is_nan() {
        max = 100.0;
    }

    max
}

fn to_float(stream: &[(u64, u64)]) -> Vec<(u64, f64)> {
    stream.iter().map(|(t, v)| (*t, *v as f64)).collect()
}

pub fn to_rates(stream: &[(u64, f64)]) -> Vec<(u64, f64)> {
    let mut result: Vec<(u64, f64)> = (0..stream.len())
        .map(|i| {
            let rate = if i > 0 {
                let bytes = stream[i].1 - stream[i - 1].1;
                let duration = Duration::from_micros(stream[i].0 - stream[i - 1].0);
                let mbits = (bytes * 8.0) / (1000.0 * 1000.0);
                mbits / duration.as_secs_f64()
            } else {
                0.0
            };
            (stream[i].0, rate)
        })
        .collect();

    // Insert dummy zero points for nicer graphs
    if !result.is_empty() {
        result.first().unwrap().0.checked_sub(1).map(|first| {
            result.insert(0, (first, 0.0));
        });
        result.push((result.last().unwrap().0 + 1, 0.0));
    }

    result
}

fn throughput(
    stream: &[(u64, f64)],
    test_data: Option<&TestData>,
    load_duration: Duration,
) -> Option<f64> {
    if stream.is_empty() {
        return None;
    }

    let start_offset = (load_duration.as_secs_f64() * 0.2).min(2.0);
    let end_offset = load_duration.as_secs_f64() - (load_duration.as_secs_f64() * 0.1).min(0.5);

    let test_start = if let Some(test_data) = test_data {
        test_data.start
    } else {
        Duration::from_micros(stream.iter().find(|e| e.1 > 0.0)?.0)
    };
    let start = (test_start + Duration::from_secs_f64(start_offset)).as_micros() as u64;
    let end = (test_start + Duration::from_secs_f64(end_offset)).as_micros() as u64;
    let end = if let Some(test_data) = test_data {
        cmp::min(test_data.end.as_micros() as u64, end)
    } else {
        end
    };

    if start >= end {
        return None;
    }

    let lookup = |point: u64| {
        let i = stream.partition_point(|e| e.0 < point);
        if i == stream.len() {
            stream[i - 1]
        } else {
            stream[i]
        }
    };

    let end = lookup(end);
    let start = lookup(start);

    let bytes = end.1 - start.1;
    let time = end.0 - start.0;
    let duration = Duration::from_micros(time).as_secs_f64();
    let mbits = (bytes * 8.0) / (1000.0 * 1000.0);
    Some(mbits / duration)
}

fn ping_peak(
    stream: &[(u64, f64)],
    test_data: Option<&TestData>,
    load_duration: Duration,
    pings: &[RawPing],
) -> Option<LatencySummary> {
    if pings.is_empty() {
        return None;
    }

    let test_start = if let Some(test_data) = test_data {
        test_data.start
    } else {
        Duration::from_micros(stream.iter().find(|e| e.1 > 0.0)?.0)
    };
    let start = test_start.as_micros() as u64;
    let end = (test_start + load_duration).as_micros() as u64;
    let end = if let Some(test_data) = test_data {
        cmp::min(test_data.end.as_micros() as u64, end)
    } else {
        end
    };

    if start >= end {
        return None;
    }

    let start = pings.partition_point(|p| (p.sent.as_micros() as u64) < start);
    let end = pings.partition_point(|p| (p.sent.as_micros() as u64) <= end);
    let values = pings.get(start..end)?;

    let point = values
        .iter()
        .max_by_key(|v| v.latency.unwrap().total.unwrap())?;

    Some(LatencySummary {
        total: point.latency.unwrap().total.unwrap(),
        down: point.latency.unwrap().down().unwrap(),
        up: point.latency.unwrap().up,
    })
}

fn ping_loss(
    stream: &[(u64, f64)],
    test_data: Option<&TestData>,
    load_duration: Duration,
    pings: &[RawPing],
) -> Option<(f64, f64)> {
    if pings.is_empty() {
        return None;
    }

    let test_start = if let Some(test_data) = test_data {
        test_data.start
    } else {
        Duration::from_micros(stream.iter().find(|e| e.1 > 0.0)?.0)
    };
    let start = test_start.as_micros() as u64;
    let end = (test_start + load_duration).as_micros() as u64;
    let end = if let Some(test_data) = test_data {
        cmp::min(test_data.end.as_micros() as u64, end)
    } else {
        end
    };

    if start >= end {
        return None;
    }

    let start = pings.partition_point(|p| (p.sent.as_micros() as u64) < start);
    let end = pings.partition_point(|p| (p.sent.as_micros() as u64) <= end);
    let values = pings.get(start..end)?;

    let loss_up = values.iter().filter(|v| v.latency.is_none()).count();

    let loss_down = values
        .iter()
        .filter(|v| v.latency.map(|l| l.total.is_none()).unwrap_or(false))
        .count();

    let count = values.len() as f64;

    Some(((loss_down as f64) / count, (loss_up as f64) / count))
}

pub fn smooth(
    stream: &[(u64, f64)],
    interval: Duration,
    smoothing_interval: Duration,
) -> Vec<(u64, f64)> {
    if stream.is_empty() {
        return Vec::new();
    }

    let interval = interval.as_micros() as u64;
    let smoothing_interval = smoothing_interval.as_micros() as u64;

    let m = cmp::max(
        1,
        ((smoothing_interval as f64 / 2.0) / (interval as f64)).ceil() as u64,
    ) as i64;
    let smoothing_interval = interval * (m as u64);

    let min = stream.first().unwrap().0.saturating_sub(smoothing_interval);
    let max = stream.last().unwrap().0 + smoothing_interval;

    let mut data = Vec::new();

    let lookup = |point: u64, m| {
        if let Some(point) = point.checked_add_signed(m * interval as i64) {
            match stream.binary_search_by_key(&point, |e| e.0) {
                Ok(i) => stream[i].1,
                Err(0) => 0.0,
                Err(i) if i == stream.len() => stream.last().unwrap().1,
                _ => panic!("unexpected index"),
            }
        } else {
            0.0
        }
    };

    for point in (min..=max).step_by(interval as usize) {
        let value = (-m..=m).map(|m| lookup(point, m)).sum::<f64>() / ((m as f64) * 2.0 + 1.0);
        data.push((point, value));
    }

    to_rates(&data)
}

fn smooth_ping(pings: &[RawPing], interval: Duration) -> Vec<RawPing> {
    if pings.is_empty() {
        return Vec::new();
    }

    let interval = interval.as_micros() as u64;
    let step = interval / 4;

    let min = (pings.first().unwrap().sent.as_micros() as u64).saturating_sub(interval);
    let max = (pings.last().unwrap().sent.as_micros() as u64) + interval;

    let mut data = Vec::new();

    for (i, point) in (min..=max).step_by(step as usize).enumerate() {
        let start =
            pings.partition_point(|p| (p.sent.as_micros() as u64) < point.saturating_sub(interval));
        let stop = pings.partition_point(|p| (p.sent.as_micros() as u64) <= point + interval);
        let values = pings.get(start..stop);

        if let Some(points) = values {
            let values: Vec<_> = points
                .iter()
                .filter_map(|v| {
                    v.latency.and_then(|l| {
                        l.total
                            .map(|total| (total.as_secs_f64(), l.up.as_secs_f64()))
                    })
                })
                .collect();
            if values.len() > 2 {
                data.push(RawPing {
                    sent: Duration::from_micros(point),
                    index: i as u64,
                    latency: Some(RawLatency {
                        total: Some(Duration::from_secs_f64(
                            values.iter().map(|v| v.0).sum::<f64>() / (values.len() as f64),
                        )),
                        up: Duration::from_secs_f64(
                            values.iter().map(|v| v.1).sum::<f64>() / (values.len() as f64),
                        ),
                    }),
                });
            }
        }
    }

    data
}

fn sum_bytes(input: &[&[(u64, f64)]], interval: Duration) -> Vec<(u64, f64)> {
    let interval = interval.as_micros() as u64;

    let throughput: Vec<_> = input
        .iter()
        .map(|stream| interpolate(stream, interval))
        .collect();

    let min = throughput
        .iter()
        .map(|stream| stream.first().map(|e| e.0).unwrap_or(0))
        .min()
        .unwrap_or(0);

    let max = throughput
        .iter()
        .map(|stream| stream.last().map(|e| e.0).unwrap_or(0))
        .max()
        .unwrap_or(0);

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let value = throughput
            .iter()
            .map(
                |stream| match stream.binary_search_by_key(&point, |e| e.0) {
                    Ok(i) => stream[i].1,
                    Err(0) => 0.0,
                    Err(i) if i == stream.len() => stream.last().unwrap().1,
                    _ => panic!("unexpected index"),
                },
            )
            .sum();
        data.push((point, value));
    }

    data
}

fn interpolate(input: &[(u64, f64)], interval: u64) -> Vec<(u64, f64)> {
    if input.is_empty() {
        return Vec::new();
    }

    let min = input.first().unwrap().0 / interval * interval;
    let max = (input.last().unwrap().0 + interval - 1) / interval * interval;

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let i = input.partition_point(|e| e.0 < point);
        let value = if i == input.len() {
            input.last().unwrap().1
        } else if input[i].0 == point || i == 0 {
            input[i].1
        } else {
            let len = input[i].0 - input[i - 1].0;
            if len == 0 {
                input[i].1
            } else {
                let ratio = (point - input[i - 1].0) as f64 / len as f64;
                let delta = input[i].1 - input[i - 1].1;
                input[i - 1].1 + delta * ratio
            }
        };
        data.push((point, value));
    }

    data
}

fn draw_centered(
    x: i32,
    y: i32,
    text: &[(String, RGBColor)],
    area: &DrawingArea<BitMapBackend<'_>, Shift>,
) {
    let small_style: TextStyle = (FontFamily::SansSerif, 14).into();

    let size: i32 = text
        .iter()
        .map(|t| area.estimate_text_size(&t.0, &small_style).unwrap().0 as i32)
        .sum::<i32>()
        / 2;

    let mut x = x + -size;

    for (text, color) in text {
        area.draw_text(
            text,
            &small_style
                .pos(Pos::new(HPos::Left, VPos::Center))
                .color(&color),
            (x, y),
        )
        .unwrap();

        x += area.estimate_text_size(text, &small_style).unwrap().0 as i32;
    }
}

fn new_chart<'a, 'c>(
    duration: f64,
    padding_bottom: Option<i32>,
    max: f64,
    label: &str,
    x_labels: bool,
    area: &'a DrawingArea<BitMapBackend<'c>, Shift>,
) -> ChartContext<'a, BitMapBackend<'c>, Cartesian2d<RangedCoordf64, RangedCoordf64>> {
    let font = (FontFamily::SansSerif, 16);

    let mut chart = ChartBuilder::on(area)
        .margin(6)
        .set_label_area_size(LabelAreaPosition::Left, 100)
        .set_label_area_size(LabelAreaPosition::Right, 100)
        .set_label_area_size(LabelAreaPosition::Bottom, padding_bottom.unwrap_or(20))
        .build_cartesian_2d(0.0..duration, 0.0..max)
        .unwrap();

    chart
        .plotting_area()
        .fill(&RGBColor(248, 248, 248))
        .unwrap();

    let mut mesh = chart.configure_mesh();

    mesh.disable_x_mesh().disable_y_mesh();

    if x_labels {
        mesh.x_labels(20).y_labels(10);
    } else {
        mesh.x_labels(0).y_labels(0);
    }

    mesh.x_label_style(font).y_label_style(font).y_desc(label);

    mesh.draw().unwrap();

    chart
}

fn legends<'a, 'b: 'a>(
    chart: &mut ChartContext<'a, BitMapBackend<'b>, Cartesian2d<RangedCoordf64, RangedCoordf64>>,
) {
    let font = (FontFamily::SansSerif, 16);

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .label_font(font)
        .border_style(BLACK)
        .draw()
        .unwrap();
}

const PACKET_LOSS_AREA_SIZE: f64 = 70.0;

fn latency<'a>(
    config: &PlotConfig,
    result: &TestResult,
    pings: &[RawPing],
    throughputs: &[ThroughputPlot],
    summary: &LatencyLossSummary,
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend<'a>, Shift>,
    packet_loss_area: Option<&DrawingArea<BitMapBackend<'a>, Shift>>,
    peer: bool,
) {
    let new_area;
    let new_packet_loss_area;
    let (packet_loss_area, area) = if let Some(packet_loss_area) = packet_loss_area {
        (packet_loss_area, area)
    } else {
        (new_area, new_packet_loss_area) =
            area.split_vertically(area.relative_to_height(1.0) - PACKET_LOSS_AREA_SIZE);
        (&new_packet_loss_area, &new_area)
    };

    // Draw latency summaries

    let small_style: TextStyle = (FontFamily::SansSerif, 14).into();

    let text_height = area.estimate_text_size("Wg", &small_style).unwrap().1 as i32 + 5;

    let center = text_height / 2 + 5;

    let side = 107;

    struct Summary {
        phase: Option<TestKind>,
        color: RGBColor,
    }

    let summaries: Vec<_> = throughputs
        .iter()
        .filter(|t| t.phase.is_some())
        .map(|throughput| Summary {
            phase: throughput.phase,
            color: throughput.color,
        })
        .chain(result.raw_result.idle().then_some(Summary {
            phase: None,
            color: RGBColor(0, 0, 0),
        }))
        .collect();

    let width =
        (area.dim_in_pixel().0.saturating_sub(side * 2) as f64 / 1.14) / (summaries.len() as f64);

    let (area, textarea) = area.split_vertically(area.dim_in_pixel().1 - (text_height as u32 + 10));

    for (i, current_summary) in summaries.iter().enumerate() {
        if let Some(latency) = summary.latencies.get(&current_summary.phase) {
            let mut text = Vec::new();

            text.push((
                format!(
                    "{}",
                    current_summary
                        .phase
                        .map(|phase| phase.name())
                        .unwrap_or("Latency")
                ),
                darken(current_summary.color, 0.5),
            ));
            text.push((
                format!(": {:.01} ms", latency.total.as_secs_f64() * 1000.0),
                RGBColor(0, 0, 0),
            ));

            text.push((
                format!("  ({:.01} ", latency.down.as_secs_f64() * 1000.0),
                RGBColor(0, 0, 0),
            ));
            text.push(("down".to_owned(), darken(DOWN_COLOR, 0.5)));
            text.push((
                format!(", {:.01} ", latency.up.as_secs_f64() * 1000.0),
                RGBColor(0, 0, 0),
            ));
            text.push(("up".to_owned(), darken(UP_COLOR, 0.5)));
            text.push((")".to_owned(), RGBColor(0, 0, 0)));

            let x = side as f64 + width * (i as f64) + width / 2.0;

            draw_centered(x.round() as i32, center, &text, &textarea);
        }
    }

    // Draw packet loss summaries

    let (packet_loss_area, textarea) =
        packet_loss_area.split_vertically(packet_loss_area.dim_in_pixel().1);

    for (i, current_summary) in summaries.iter().enumerate() {
        if let Some(&(down, up)) = summary.loss.get(&current_summary.phase) {
            let mut text = Vec::new();

            text.push((
                format!(
                    "{}",
                    current_summary
                        .phase
                        .map(|phase| phase.name())
                        .unwrap_or("Packet loss")
                ),
                darken(current_summary.color, 0.5),
            ));
            if down == 0.0 && up == 0.0 {
                text.push((": 0%".to_owned(), RGBColor(0, 0, 0)));
            } else {
                text.push((
                    format!(": {:.1$}% ", down * 100.0, if down == 0.0 { 0 } else { 2 }),
                    RGBColor(0, 0, 0),
                ));
                text.push(("down".to_owned(), darken(DOWN_COLOR, 0.5)));
                text.push((
                    format!(", {:.1$}% ", up * 100.0, if up == 0.0 { 0 } else { 2 }),
                    RGBColor(0, 0, 0),
                ));
                text.push(("up".to_owned(), darken(UP_COLOR, 0.5)));
            }

            let x = side as f64 + width * (i as f64) + width / 2.0;

            draw_centered(x.round() as i32, -16, &text, &textarea);
        }
    }

    // Draw latency plot

    let max_latency = pings
        .iter()
        .filter_map(|d| d.latency)
        .filter_map(|latency| latency.total)
        .max()
        .unwrap_or(Duration::from_millis(100))
        .as_secs_f64()
        * 1000.0;

    let mut max_latency = max_latency * 1.05;

    if let Some(max) = config.max_latency.map(|l| l as f64) {
        if max > max_latency {
            max_latency = max;
        }
    }

    let mut chart = new_chart(
        duration,
        None,
        max_latency,
        if peer {
            "Peer latency (ms)"
        } else {
            "Latency (ms)"
        },
        true,
        &area,
    );

    let mut draw_latency =
        |color: RGBColor, name: &str, get_latency: fn(&RawLatency) -> Option<Duration>| {
            let mut data = Vec::new();

            let flush = |data: &mut Vec<_>| {
                let data = mem::take(data);

                if data.len() == 1 {
                    chart
                        .plotting_area()
                        .draw(&Circle::new(data[0], 1, color.filled()))
                        .unwrap();
                } else {
                    chart
                        .plotting_area()
                        .draw(&PathElement::new(data, color))
                        .unwrap();
                }
            };

            for ping in pings {
                match &ping.latency {
                    Some(latency) => match get_latency(latency) {
                        Some(latency) => {
                            let x = ping.sent.as_secs_f64() - start;
                            let y = latency.as_secs_f64() * 1000.0;

                            data.push((x, y));
                        }
                        None => {
                            flush(&mut data);
                        }
                    },
                    None => {
                        flush(&mut data);
                    }
                }
            }

            flush(&mut data);

            chart
                .draw_series(LineSeries::new(std::iter::empty(), color))
                .unwrap()
                .label(name)
                .legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 18, y + 3)], color.filled())
                });
        };

    draw_latency(UP_COLOR, "Up", |latency| Some(latency.up));

    draw_latency(DOWN_COLOR, "Down", |latency| latency.down());

    draw_latency(RGBColor(50, 50, 50), "Round-trip", |latency| latency.total);

    legends(&mut chart);

    // Packet loss

    let chart = new_chart(
        duration,
        Some(30),
        1.0,
        if peer { "Peer loss" } else { "Packet loss" },
        false,
        &packet_loss_area,
    );

    for ping in pings {
        let x = ping.sent.as_secs_f64() - start;
        if ping.latency.and_then(|latency| latency.total).is_none() {
            let bold_size = 0.1111;
            let (color, s, e, bold) = if result.raw_result.version >= 2 {
                if ping.latency.is_none() {
                    (UP_COLOR, 0.0, 0.5, Some(0.0 + bold_size))
                } else {
                    (DOWN_COLOR, 1.0, 0.5, Some(1.0 - bold_size))
                }
            } else {
                (RGBColor(193, 85, 85), 0.0, 1.0, None)
            };
            chart
                .plotting_area()
                .draw(&PathElement::new(vec![(x, s), (x, e)], color))
                .unwrap();
            bold.map(|bold| {
                chart
                    .plotting_area()
                    .draw(&PathElement::new(
                        vec![(x, s), (x, bold)],
                        color.stroke_width(2),
                    ))
                    .unwrap();
            });
        }
    }

    chart
        .plotting_area()
        .draw(&PathElement::new(vec![(0.0, 1.0), (duration, 1.0)], BLACK))
        .unwrap();
}

fn plot_split_throughput(
    config: &PlotConfig,
    download: bool,
    result: &TestResult,
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend, Shift>,
) {
    let groups: Vec<_> = result
        .stream_groups
        .iter()
        .filter(|group| group.download == download)
        .map(|group| TestStreamGroup {
            download,
            both: group.both,
            streams: group
                .streams
                .iter()
                .map(|stream| TestStream {
                    data: to_rates(&stream.data),
                })
                .collect(),
        })
        .collect();

    let max_throughput = float_max(
        groups
            .iter()
            .flat_map(|group| group.streams.last().unwrap().data.iter())
            .map(|e| e.1),
    );

    let mut max_throughput = max_throughput * 1.05;

    if let Some(max) = config.max_throughput.map(|l| l as f64 / (1000.0 * 1000.0)) {
        if max > max_throughput {
            max_throughput = max;
        }
    }

    let mut chart = new_chart(
        duration,
        None,
        max_throughput,
        if download {
            "Download (Mbps)"
        } else {
            "Upload (Mbps)"
        },
        true,
        area,
    );

    for group in groups {
        for i in 0..(group.streams.len()) {
            let main = i == group.streams.len() - 1;
            let color = if download {
                if main {
                    DOWN_COLOR
                } else {
                    if i & 1 == 0 {
                        RGBColor(188, 203, 177)
                    } else {
                        RGBColor(215, 223, 208)
                    }
                }
            } else {
                if main {
                    UP_COLOR
                } else {
                    if i & 1 == 0 {
                        RGBColor(159, 172, 202)
                    } else {
                        RGBColor(211, 217, 231)
                    }
                }
            };
            chart
                .draw_series(LineSeries::new(
                    group.streams[i].data.iter().map(|(time, rate)| {
                        (Duration::from_micros(*time).as_secs_f64() - start, *rate)
                    }),
                    color,
                ))
                .unwrap();
        }
    }
}

fn plot_throughput(
    config: &PlotConfig,
    throughputs: &[ThroughputPlot],
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend<'_>, Shift>,
) {
    let max_throughput = float_max(
        throughputs
            .iter()
            .flat_map(|list| list.rates.iter())
            .map(|e| e.1),
    );

    let mut max_throughput = max_throughput * 1.05;

    if let Some(max) = config.max_throughput.map(|l| l as f64 / (1000.0 * 1000.0)) {
        if max > max_throughput {
            max_throughput = max;
        }
    }

    let small_style: TextStyle = (FontFamily::SansSerif, 14).into();

    let text_height = area.estimate_text_size("Wg", &small_style).unwrap().1 as i32 + 5;

    let center = text_height / 2 + 5;

    let side = 107;
    let width = (area.dim_in_pixel().0.saturating_sub(side * 2) as f64 / 1.14)
        / (throughputs.iter().filter(|t| t.phase.is_some()).count() as f64);

    let (area, textarea) = area.split_vertically(area.dim_in_pixel().1 - (text_height as u32 + 10));

    for (i, throughput) in throughputs.iter().filter(|t| t.phase.is_some()).enumerate() {
        if let Some(rate) = throughput.rate {
            let mut text = Vec::new();

            text.push((
                format!("{}", throughput.phase.unwrap().name()),
                darken(throughput.color, 0.5),
            ));
            text.push((format!(": {:.02} Mbps", rate), RGBColor(0, 0, 0)));

            if let Some((down, up)) = throughput.dual_rates {
                text.push((format!("  ({:.02} ", down), RGBColor(0, 0, 0)));
                text.push(("down".to_owned(), darken(DOWN_COLOR, 0.5)));
                text.push((format!(", {:.02} ", up), RGBColor(0, 0, 0)));
                text.push(("up".to_owned(), darken(UP_COLOR, 0.5)));
                text.push((")".to_owned(), RGBColor(0, 0, 0)));
            }

            let x = side as f64 + width * (i as f64) + width / 2.0;

            draw_centered(x.round() as i32, center, &text, &textarea);
        }
    }

    let mut chart = new_chart(
        duration,
        None,
        max_throughput,
        "Throughput (Mbps)",
        true,
        &area,
    );

    let mut seen = HashSet::new();
    for throughput in throughputs {
        let series = chart
            .draw_series(LineSeries::new(
                throughput.rates.iter().map(|(time, rate)| {
                    (Duration::from_micros(*time).as_secs_f64() - start, *rate)
                }),
                throughput.color,
            ))
            .unwrap();
        if seen.insert(throughput.name.to_owned()) {
            series.label(throughput.name).legend(move |(x, y)| {
                Rectangle::new([(x, y - 5), (x + 18, y + 3)], throughput.color.filled())
            });
        }
    }

    for throughput in throughputs {
        chart
            .draw_series(LineSeries::new(
                throughput.smooth.iter().map(|(time, rate)| {
                    (Duration::from_micros(*time).as_secs_f64() - start, *rate)
                }),
                ShapeStyle {
                    color: darken(throughput.color, 0.5).mix(0.5),
                    filled: true,
                    stroke_width: 2,
                },
            ))
            .unwrap();
    }

    legends(&mut chart);
}

pub(crate) fn bytes_transferred(
    throughputs: &[ThroughputPlot],
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend, Shift>,
) {
    let max_bytes = float_max(
        throughputs
            .iter()
            .flat_map(|list| list.bytes.iter())
            .flat_map(|list| list.iter())
            .map(|e| e.1),
    );

    let max_bytes = max_bytes / (1024.0 * 1024.0 * 1024.0);

    let max_bytes = max_bytes * 1.05;

    let mut chart = new_chart(
        duration,
        Some(50),
        max_bytes,
        "Data transferred (GiB)",
        true,
        area,
    );

    let mut seen = HashSet::new();
    for throughput in throughputs {
        for (i, bytes) in throughput.bytes.iter().enumerate() {
            let series = chart
                .draw_series(LineSeries::new(
                    bytes.iter().map(|(time, bytes)| {
                        (
                            Duration::from_micros(*time).as_secs_f64() - start,
                            *bytes / (1024.0 * 1024.0 * 1024.0),
                        )
                    }),
                    &throughput.color,
                ))
                .unwrap();

            if seen.insert(throughput.name.to_owned()) && i == 0 {
                series.label(throughput.name).legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 18, y + 3)], throughput.color.filled())
                });
            }
        }
    }

    legends(&mut chart);
}

pub(crate) fn graph(
    config: &PlotConfig,
    result: &TestResult,
    pings: &[RawPing],
    throughput: &[ThroughputPlot],
    start: f64,
    duration: f64,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, anyhow::Error> {
    let width = config.width.unwrap_or(1280) as u32;

    let peer_latency = result.raw_result.peer_pings.is_some();

    let mut def_height = 720;

    if peer_latency {
        def_height += 380;
    }

    if config.transferred {
        def_height += 320;
    }

    let height = config.height.unwrap_or(def_height) as u32;

    let mut data = vec![0; 3 * (width as usize * height as usize)];

    let idle = result.raw_result.idle();

    let title = config.title.as_deref().unwrap_or(if idle {
        "Latency"
    } else {
        "Latency under load"
    });

    {
        let root = BitMapBackend::with_buffer(&mut data, (width, height)).into_drawing_area();

        root.fill(&WHITE).unwrap();

        let style: TextStyle = (FontFamily::SansSerif, 26).into();

        let medium_style: TextStyle = (FontFamily::SansSerif, 16).into();

        let small_style: TextStyle = (FontFamily::SansSerif, 14).into();

        let lines = 2;

        let text_height =
            (root.estimate_text_size("Wg", &small_style).unwrap().1 as i32 + 5) * lines;

        let center = text_height / 2 + 10;

        root.draw_text(
            title,
            &style.pos(Pos::new(HPos::Center, VPos::Center)),
            (width as i32 / 2, center),
        )
        .unwrap();

        if result.raw_result.version >= 1 {
            let top_margin = 10;
            root.draw_text(
                &format!(
                    "Connections: {} over IPv{}",
                    result.raw_result.streams(),
                    if result.raw_result.ipv6 { 6 } else { 4 },
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100, top_margin + text_height / lines),
            )
            .unwrap();

            root.draw_text(
                &format!(
                    "Stagger: {} s",
                    result.raw_result.config.stagger.as_secs_f64(),
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100 + 180, top_margin + text_height / lines),
            )
            .unwrap();

            root.draw_text(
                &if idle {
                    format!(
                        "Grace duration: {:.2} s",
                        result.raw_result.config.grace_duration.as_secs_f64(),
                    )
                } else {
                    format!(
                        "Load duration: {:.2} s",
                        result.raw_result.config.load_duration.as_secs_f64(),
                    )
                },
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100, top_margin),
            )
            .unwrap();

            root.draw_text(
                &format!(
                    "Server latency: {:.2} ms",
                    result.raw_result.server_latency.as_secs_f64() * 1000.0,
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100 + 180, top_margin),
            )
            .unwrap();

            root.draw_text(
                &result.raw_result.generated_by,
                &small_style.pos(Pos::new(HPos::Right, VPos::Center)),
                (width as i32 - 100, center),
            )
            .unwrap();
        }

        let (root, textarea) = root.split_vertically(root.dim_in_pixel().1 - 24);

        textarea
            .draw_text(
                "Elapsed time (seconds)",
                &medium_style.pos(Pos::new(HPos::Center, VPos::Center)),
                ((width as i32) / 2, 12),
            )
            .unwrap();

        let mut root = root.split_vertically(text_height + 10).1;

        let loss = if !peer_latency {
            let loss;
            (root, loss) =
                root.split_vertically(root.relative_to_height(1.0) - PACKET_LOSS_AREA_SIZE);
            Some(loss)
        } else {
            None
        };

        let mut charts = 1;

        if peer_latency {
            charts += 1;
        }

        if result.raw_result.streams() > 0 {
            if config.split_throughput {
                if result.raw_result.download() || result.raw_result.both() {
                    charts += 1
                }
                if result.raw_result.upload() || result.raw_result.both() {
                    charts += 1
                }
            } else {
                charts += 1
            }
            if config.transferred {
                charts += 1
            }
        }

        let areas = root.split_evenly((charts, 1));

        // Scale to fit the legend
        let duration = duration * 1.12;

        let mut chart_index = 0;

        if result.raw_result.streams() > 0 {
            if config.split_throughput {
                if result.raw_result.download() || result.raw_result.both() {
                    plot_split_throughput(
                        config,
                        true,
                        result,
                        start,
                        duration,
                        &areas[chart_index],
                    );
                    chart_index += 1;
                }
                if result.raw_result.upload() || result.raw_result.both() {
                    plot_split_throughput(
                        config,
                        false,
                        result,
                        start,
                        duration,
                        &areas[chart_index],
                    );
                    chart_index += 1;
                }
            } else {
                plot_throughput(config, throughput, start, duration, &areas[chart_index]);
                chart_index += 1;
            }
        }

        latency(
            config,
            result,
            pings,
            throughput,
            &result.latencies,
            start,
            duration,
            &areas[chart_index],
            loss.as_ref(),
            false,
        );
        chart_index += 1;

        if let Some(peer_pings) = result.raw_result.peer_pings.as_ref() {
            latency(
                config,
                result,
                peer_pings,
                throughput,
                &result.peer_latencies,
                start,
                duration,
                &areas[chart_index],
                None,
                true,
            );
            chart_index += 1;
        }

        if result.raw_result.streams() > 0 && config.transferred {
            bytes_transferred(throughput, start, duration, &areas[chart_index]);
            #[allow(unused_assignments)]
            {
                chart_index += 1;
            }
        }

        root.present().map_err(|_| anyhow!("Unable to plot"))?;
    }

    ImageBuffer::from_raw(width, height, data).ok_or(anyhow!("Failed to create image"))
}
