//! Check that a pre-split config.json is carried across intact.
fn main() {
    let old = r##"{
      "discord": { "client_id": "123456", "client_secret": "s3cret" },
      "appearance": {
        "background": "#11223344",
        "speaking": "#00FF00",
        "height": 40,
        "avatar_size": 28,
        "avatar_overlap": -4,
        "max_avatars": 9,
        "middle_click": "volume_reset",
        "max_volume": 400,
        "show_guild_name": true,
        "monitors": ["all"]
      }
    }"##;

    let config: discord_taskbar::config::Config =
        serde_json::from_str(&reserialise(old)).expect("parse");

    let theme = discord_taskbar::ui::theme::Theme::from(&config.appearance);
    let stored: discord_taskbar::integration::discord::settings::Stored =
        serde_json::from_value(config.integration("discord")).expect("discord section");
    let settings = discord_taskbar::integration::discord::settings::Settings::from(&stored);

    println!("client_id       {}", stored.client_id);
    println!("client_secret   {}", stored.client_secret);
    println!("accent (was speaking) {:?}", theme.accent);
    println!("background      {:?}", theme.background);
    println!("height          {}", theme.height);
    println!("monitors        {:?}", theme.monitors);
    println!("avatar_size     {}", settings.avatar_size);
    println!("avatar_overlap  {}", settings.avatar_overlap);
    println!("max_avatars     {}", settings.max_avatars);
    println!("middle_click    {:?}", settings.middle_click);
    println!("max_volume      {}", settings.max_volume);
    println!("show_guild_name {}", settings.show_guild_name);
}

/// `Config::load_or_create` reads from disk; this exercises the same
/// migration by round-tripping the text through it.
fn reserialise(old: &str) -> String {
    let dir = std::env::temp_dir().join("dt-migrate-check");
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single-threaded probe.
    unsafe {
        std::env::set_var("APPDATA", &dir);
    }
    let path = discord_taskbar::config::config_dir().join("config.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, old).unwrap();

    let (config, warning) = discord_taskbar::config::Config::load();
    if let Some(w) = warning {
        eprintln!("warning: {w}");
    }
    serde_json::to_string(&config).unwrap()
}
