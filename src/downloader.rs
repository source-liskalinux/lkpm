use anyhow::{Context, Result};
use reqwest::blocking::Client;
use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use crate::config::Config;
use crate::error::LkpmError;
use crate::repo;

fn http_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent("lkpm/1.1.0-1")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?)
}

pub fn download_to(
    url: &str,
    dest: &Path,
    pb: Option<&ProgressBar>,
    overall_pb: Option<&ProgressBar>,
) -> anyhow::Result<PathBuf> {
    let client = http_client()?;
    let max_attempts = 3;
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=max_attempts {
        if attempt > 1 {
            if let Some(pb) = pb {
                pb.set_position(0);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        match download_once(&client, url, dest, pb, overall_pb) {
            Ok(path) => {
                if let Some(pb) = pb {
                    pb.finish_and_clear();
                }
                return Ok(path);
            }
            Err(err) => {
                if dest.exists() {
                    let _ = fs::remove_file(dest);
                }
                last_error = Some(err);
            }
        }
    }
    if let Some(pb) = pb {
        pb.finish_and_clear();
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Failed to download after {} attemps!", max_attempts)))
        .with_context(|| format!("Failed to download from {url}!"))
}

fn download_once(
    client: &Client,
    url: &str,
    dest: &Path,
    pb: Option<&ProgressBar>,
    overall_pb: Option<&ProgressBar>,
) -> anyhow::Result<PathBuf> {
    let resp = client.get(url).send().with_context(|| format!("Failed to send request to {url}!"))?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP error status: {}", resp.status());
    }
    let total_size = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if let Some(pb) = pb {
        if total_size > 0 {
            pb.set_length(total_size);
        }
    }
    let mut out = File::create(dest).with_context(|| format!("Failed to create {} file!", dest.display()))?;
    let mut source = resp;
    let mut downloaded_bytes: u64 = 0;
    let mut buf = [0u8; 4 * 1024];
    loop {
        let n = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if let Some(opb) = overall_pb {
                    if downloaded_bytes > 0 {
                        opb.dec(downloaded_bytes);
                    }
                }
                return Err(anyhow::Error::from(e)).context("Connection disconnected when trying to download the data!");
            }
        };
        out.write_all(&buf[..n])?;
        let chunk_size = n as u64;
        downloaded_bytes += chunk_size;
        if let Some(pb) = pb {
            pb.inc(chunk_size);
        }
        if let Some(opb) = overall_pb {
            opb.inc(chunk_size);
        }
    }
    out.flush()?;
    if total_size > 0 && downloaded_bytes != total_size {
        if let Some(opb) = overall_pb {
            if downloaded_bytes > 0 {
                opb.dec(downloaded_bytes);
            }
        }
        anyhow::bail!(
            "Downloaded file size not matched with the actual file size! ({}/{} bytes)",
            downloaded_bytes,
            total_size
        );
    }
    Ok(dest.to_path_buf())
}

pub fn download_packages_concurrently(
    cfg: &Config,
    urls: &[String],
) -> Result<Vec<Option<PathBuf>>, LkpmError> {
    let total = urls.len();
    if total == 0 {
        return Ok(Vec::new());
    }
    let mp = Arc::new(MultiProgress::new());
    let pb_style = ProgressStyle::default_bar()
        .template("{spinner:.bright.green} {bar:50.bright.cyan/blue} {percent:>3}% [ {bytes:>11} | {total_bytes:>11} | {eta_precise} ] {msg}")
        .unwrap()
        .progress_chars("▓▒░")
        .tick_chars("•◦");
    let overall_pb = mp.add(ProgressBar::new(total as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("\n{spinner:.bright.green} [ {bytes:>11} | {binary_bytes_per_sec:>13} ] {len} packages")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
    );
    overall_pb.enable_steady_tick(Duration::from_millis(80));
    let queue: Arc<Mutex<VecDeque<(usize, String)>>> = Arc::new(Mutex::new(
        urls.iter().cloned().enumerate().collect()
    ));
    let (tx, rx) = mpsc::channel();
    let concurrency_limit = cfg.parallel_operation.max(1);
    let num_workers = concurrency_limit.min(total);
    for _ in 0..num_workers {
        let queue_clone = Arc::clone(&queue);
        let tx_clone = tx.clone();
        let cfg_clone = cfg.clone();
        let mp_clone = Arc::clone(&mp);
        let overall_pb_clone = overall_pb.clone();
        let style_clone = pb_style.clone();
        std::thread::spawn(move || {
            loop {
                let task = {
                    let mut q = queue_clone.lock().unwrap();
                    q.pop_front()
                };
                let (index, url) = match task {
                    Some(t) => t,
                    None => break,
                };
                let file_name = repo::package_file_name_from_url(&url);
                let pkg_name = repo::package_name_from_file_name(&file_name);
                let pb = mp_clone.insert_before(&overall_pb_clone, ProgressBar::new(0));
                pb.set_style(style_clone.clone());
                pb.set_message(pkg_name);
                let res = repo::download_pkg_url(
                    &cfg_clone, 
                    &url, 
                    Some(pb.clone()), 
                    Some(overall_pb_clone.clone())
                );
                pb.finish_and_clear();
                mp_clone.remove(&pb);
                overall_pb_clone.inc(1);
                if tx_clone.send((index, res)).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let mut results = vec![None; total];
    for (index, res) in rx {
        match res {
            Ok(path) => results[index] = Some(path),
            Err(..) => {
                continue;
            }
        }
    }
    overall_pb.finish_and_clear();
    mp.clear().ok();
    Ok(results)
}
