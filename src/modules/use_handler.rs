use crate::models::{StableVersion, DownloadedFile};
use anyhow::{anyhow, Result};
use clap::ArgMatches;
use dirs::data_local_dir;
use futures_util::stream::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use reqwest::Client;
use std::cmp::min;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub async fn start(command: &ArgMatches) -> Result<()> {
    let client = Client::new();
    let version = if let Some(value) = command.value_of("VERSION") {
        match parse_version(&client, value).await {
            Ok(version) => version,
            Err(error) => return Err(anyhow!(error)),
        }
    } else {
        return Err(anyhow!("Todo.."));
    };

    let downloaded_version = match download_version(&client, &version).await {
        Ok(value) => value,
        Err(error) => return Err(anyhow!(error)),
    };

    if let Err(error) = install_version(downloaded_version).await {
        return Err(anyhow!(error));
    }

    Ok(())
}

async fn parse_version(client: &Client, version: &str) -> Result<String> {
    match version {
        "nightly" => Ok(String::from(version)),
        "stable" => {
            let response = client
                .get("https://api.github.com/repos/neovim/neovim/releases/latest")
                .header("user-agent", "bob")
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await?
                .text()
                .await?;

            let latest: StableVersion = serde_json::from_str(response.as_str())?;

            Ok(latest.tag_name)
        }
        _ => {
            let regex = Regex::new(r"^v?[0-9]+\.[0-9]+\.[0-9]+$").unwrap();
            if regex.is_match(version) {
                let mut returned_version = String::from(version);
                if !version.contains('v') {
                    returned_version.insert(0, 'v');
                }
                return Ok(returned_version);
            }
            Err(anyhow!("Please provide a proper version string"))
        }
    }
}

async fn download_version(client: &Client, version: &String) -> Result<DownloadedFile> {
    let response = send_request(client, version).await;

    match response {
        Ok(response) => {
            if response.status() == 200 {
                let total_size = response.content_length().unwrap();
                let mut response_bytes = response.bytes_stream();

                // Progress Bar Setup
                let pb = ProgressBar::new(total_size);
                pb.set_style(ProgressStyle::default_bar()
                    .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                    .progress_chars("█  "));
                pb.set_message(format!("Downloading version: {version}"));

                let downloads_dir = match get_downloads_folder().await {
                    Ok(value) => value,
                    Err(error) => return Err(anyhow!(error)),
                };
                let downloads_dir_str = downloads_dir.to_str().unwrap();
                let file_type = get_file_type();
                let mut file =
                    tokio::fs::File::create(format!("{downloads_dir_str}/{version}.{file_type}"))
                        .await?;

                let mut downloaded: u64 = 0;

                while let Some(item) = response_bytes.next().await {
                    let chunk = item.or(anyhow::private::Err(anyhow::Error::msg("hello")))?;
                    file.write(&chunk).await;
                    let new = min(downloaded + (chunk.len() as u64), total_size);
                    downloaded = new;
                    pb.set_position(new);
                }

                pb.finish_with_message(format!(
                    "Downloaded version {version} to {downloads_dir_str}/{version}.{file_type}"
                ));

                Ok(DownloadedFile {
                    path: downloads_dir,
                    extension: file_type,
                    name: String::from(version),
                })
            } else {
                Err(anyhow!("Please provide an existing neovim version"))
            }
        }
        Err(error) => Err(anyhow!(error)),
    }
}

async fn send_request(
    client: &Client,
    version: &String,
) -> Result<reqwest::Response, reqwest::Error> {
    let os = if cfg!(target_os = "linux") {
        "linux64"
    } else if cfg!(target_os = "windows") {
        "win64"
    } else {
        "macos"
    };
    let request_url = format!(
        "https://github.com/neovim/neovim/releases/download/{version}/nvim-{os}.{}",
        get_file_type()
    );

    client
        .get(request_url)
        .header("user-agent", "bob")
        .send()
        .await
}

fn get_file_type() -> String {
    if cfg!(target_family = "windows") {
        String::from("zip")
    } else {
        String::from("tar.gz")
    }
}

async fn get_downloads_folder() -> Result<PathBuf> {
    let data_dir = match data_local_dir() {
        None => return Err(anyhow!("Couldn't get data folder")),
        Some(value) => value,
    };
    let path_string = format!("{}/bob", data_dir.to_str().unwrap());
    let does_folder_exist = tokio::fs::metadata(path_string.clone()).await.is_ok();

    if !does_folder_exist {
        if let Err(error) = tokio::fs::create_dir(path_string.clone()).await {
            return Err(anyhow!(error));
        }
    }
    Ok(PathBuf::from(path_string))
}

async fn install_version(downloaded_file: DownloadedFile) -> Result<()> {
    println!("Installing");
    let output = if cfg!(target_os = "windows") {
        Command::new("powershell")
            .current_dir(downloaded_file.path)
            .arg("-c")
            .arg(format!(
                "\
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [System.IO.Compression.ZipFile]::ExtractToDirectory(\"{}.{}\", \"./{0}\")
        ", downloaded_file.name, downloaded_file.extension))
            .output()
            .await?
    } else {
        Command::new("bash")
            .current_dir(downloaded_file.path)
            .arg("-c")
            .arg(format!(
                "\
            tar -xf {}
            ",
                downloaded_file.name
            ))
            .output()
            .await?
    };
    if !output.status.success() {
        return Err(anyhow!(
            "Failed to uncompress {} {}",
            downloaded_file.name,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    println!("Finsihed installing");

    Ok(())
}
