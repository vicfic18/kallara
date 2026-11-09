
<div align="center">
  <img width="500" src="kallara-logo.svg" alt="Kallara Logo">
</div>

> **kallara** (kʌl-lʌ-rʌ): underground stone vault used to safeguard royal treasures and/or dead bodies.

Minimal *(~4 MB)* tool for creating encrypted *(AES 128 GCM)* compressed *(zstd)* archives *(tar)*.

Just drag and drop your files, click archive, and give it a location. Copy it, chop it up, 

## FAQ

<details>
<summary><b>What is this for?</b></summary>

For the files that you don't want to just leave in a plain folder in your system. Full disk encryption is useless after you unlock your system, and virtual disk encrypted drives are not exactly cross platform. 

*Example 1*: You are using a shared computer (family, instituitional, etc). And want to keep your files persistantly on the system without other users accessing it.

*Example 2*: You want to share a file sharing device (penddrive, external hard disk, etc) and don't want the recipient to see the other files that are there.

Sensitive files like *bank statements, government Documents,* etc are the kind of files.
</details>

<details>
<summary><b>Why not any existing apps?</b></summary>

Kallara is small *(~4 MB)*, and cross platform. The closest alternative is [cryptomator](https://cryptomator.org/), but it  doesn't support compression, whereas kallara has *zstd*, the most [pareto optimal](https://insanity.industries/post/pareto-optimal-compression/) compression format. zstd has the best balance between speed and compression ratio, and there aren't a lot of archival apps which takes advantage of that.

And ofcourse this app is written in Rust. hehe.
</details>

<details>
<summary><b>How is it so small?</b></summary>

We use [dioxus](https://dioxuslabs.com/) framework for the UI, which uses the native web view libaries of each platform. Which is faster then any electron app.
</details>

## Prerequisites

Tested on Arch Linux running GNOME 51.1 with Wayland.

```bash
sudo pacman -S --needed webkit2gtk gtk3 xdotool
```

## Development

```bash
# Run tests across workspace
cargo test --workspace

# Run core tests with logs
cargo test -p kallara-core -- --nocapture

# Run development GUI
cargo run -p kallara-ui
```

## Production Build

```bash
cargo build --release
# target/release/kallara-ui
```

## Credits

Varghese K James - [vicfic.xyz](https://vicfic.xyz)

Code published under AGPL v3.

Redaction 35 Font under [SIL Open Font License, Version 1.1](https://openfontlicense.org/open-font-license-official-text/)
