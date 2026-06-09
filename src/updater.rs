use anyhow::Result;
use serde::Deserialize;
use std::env;
use std::fs;

const REPO_URL: &str = "https://api.github.com/repos/congchuahiep/win-glide/releases/latest";

#[derive(Deserialize, Debug)]
pub struct Release {
    pub tag_name: String,
    pub body: Option<String>,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize, Debug)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub download_url: String,
    pub release_notes: Option<String>,
}

pub fn check_for_updates() -> Result<Option<UpdateInfo>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("WinGlide-Updater")
        .build()?;

    let response = client.get(REPO_URL).send()?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch release info: {}", response.status());
    }

    let release: Release = response.json()?;
    let current_version = env!("CARGO_PKG_VERSION");

    // Lấy tag_name (loại bỏ 'v' ở đầu nếu có)
    let latest_version = release.tag_name.trim_start_matches('v').to_string();

    // So sánh version đơn giản
    if latest_version != current_version {
        // Tìm file MSI trong assets
        if let Some(asset) = release.assets.iter().find(|a| a.name.ends_with(".msi")) {
            return Ok(Some(UpdateInfo {
                latest_version,
                download_url: asset.browser_download_url.clone(),
                release_notes: release.body.clone(),
            }));
        }
    }

    Ok(None)
}

pub fn download_and_install(url: &str) -> Result<()> {
    let mut temp_dir = env::temp_dir();
    temp_dir.push("winglide_update.msi");

    let client = reqwest::blocking::Client::builder()
        .user_agent("WinGlide-Updater")
        .build()?;

    let mut response = client.get(url).send()?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to download update: {}", response.status());
    }

    let mut dest = fs::File::create(&temp_dir)?;
    response.copy_to(&mut dest)?;

    // Chạy file msi bằng ShellExecuteW
    unsafe {
        use windows::core::PCWSTR;
        use windows::core::w;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let path_wide: Vec<u16> = temp_dir
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let _ = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }

    Ok(())
}
