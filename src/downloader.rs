use anyhow::{Context, Result};
use reqwest::blocking::Client;
use std::collections::VecDeque;
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
        .timeout(Duration::from_secs(20))
        .build()?)
}

pub fn download_to(
    url: &str,
    dest: &Path,
    pb: Option<&ProgressBar>,
) -> anyhow::Result<PathBuf> {
    let client = http_client()?;
    let mut attempt = 0;
    let resp = loop {
        attempt += 1;
        match client.get(url).send() {
            Ok(resp) => break resp,
            Err(err) => {
                if attempt >= 3 || !(err.is_timeout() || err.is_connect() || err.is_request()) {
                    return Err(err).with_context(|| format!("Failed sending request to {url}!"));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    if !resp.status().is_success() {
        anyhow::bail!("HTTP error: {}", resp.status());
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
    let mut out = File::create(dest).with_context(|| format!("Failed to create {}!", dest.display()))?;
    let mut source = resp;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        if let Some(pb) = pb {
            pb.inc(n as u64);
        }
    }
    if let Some(pb) = pb {
        pb.finish_and_clear();
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
    let overall_pb = mp.add(ProgressBar::new(total as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.bright.green} {percent:>3}% [{pos}/{len}] package downloaded")
            .unwrap()
            .progress_chars("▓▒░")
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
                let pb = mp_clone.add(ProgressBar::new(0));
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{spinner:.bright.green} {bar:40.bright.cyan/blue} {percent:>3}% [{bytes:>10}/{total_bytes:>10}] {msg}")
                        .unwrap()
                        .progress_chars("▓▒░")
                        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                );
                pb.set_message(pkg_name);
                let res = repo::download_pkg_url(&cfg_clone, &url, Some(pb));
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
            Err(e) => {
                eprintln!("Warning: Gagal mendownload package #{}. Skip langsung! (Error: {})", index, e);
            }
        }
    }
    overall_pb.finish_and_clear();
    Ok(results)
}
