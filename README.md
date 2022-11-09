<div align="center">
  <h1>🎥 filmweb-export-rs</h1>
  <p>export 🚀blazingly fast🚀 Filmweb's ratings to a TMDB import-compatible csv file 😱 (IMDb v3 to be exact)</p>
  <a><img src="https://user-images.githubusercontent.com/73398428/200909825-9195f6b9-4e37-43ab-82a4-127edf27e6c0.png" height="594" width="1061"></a>
</div>

# Table of contents
- [Features](#features)
- [Installation](#installation)
- [Obtaining cookies](#obtaining-cookies)
- [Usage](#usage)
  - [Example usages](#examples)
- [Troubleshooting](#troubleshooting)

# Features:

- TMDB.org compatible format (IMDb v3)
- Exports favorited, films, serials and marked to see
- Multi-threaded
- Accuracy

# Installation

  ```
  $ git clone https://github.com/micielski/filmweb-export-rs.git  
  $ cd filmweb-export-rs
  $ cargo run --release
  ```
# Obtaining cookies
  
  1. Go to a page which requires authentication (i.e. not filmweb main page, but your profile https://filmweb.pl/user/YOUR_USERNAME)
  2. Open developer tools with F12
  3. On Firefox, navigate to the Storage tab, on Chromium; Application tab
  4. For both browsers go to Cookies and select "https://filmweb.pl"
  5. You're searching for cookies named \_fwuser_token, \_fwuser_session, JWT. Their values are needed

# Usage
 

  ```
$ cargo run --release -- --help
Finished release [optimized] target(s) in 1.12s
Running `target/release/filmweb-export-rs --help`
filmweb-export starting...
Exports user data from filmweb.pl to IMDBv3 csv file format

Usage: filmweb-export-rs [OPTIONS]

Options:
  -u, --username <USERNAME>
  -t, --token <TOKEN>        _fwuser_token cookie value
  -s, --session <SESSION>    _fwuser_sessionId cookie value
  -j, --jwt <JWT>            JWT cookie value
  --threads <THREADS>    Number of threads to spawn [default: 6]
  -q, --quiet                If enabled, successfully exported titles won't be printed
  -h, --help                 Print help information
  -V, --version              Print version information
  ```

## Examples

  #### Login with cookies  
  You'll be prompted for cookies if you won't specify these with flags.

  ```
  $ cargo run --release
  ```
    
  #### --threads <THREADS> 
  It may be useful if your internet connection or computer at this time is poor in resources. In that case try lowering threads count.  
  Please note that the default value is high enough. Going above that, it will hurt your exporting performance, and even cause some movies to not export!
  
  ```
  $ cargo run --release --  --threads 2
  ```
    
  #### Debug  
  If you are experiencing somethings abnormal, such as panics or something and you're experienced enough with rust - you may wish to enable debug printing mode
  
  ```
  $ RUST_LOG=debug cargo run --release
  ```

# Troubleshooting
  
Try lowering the threads count, otherwise if it won't help, open an issue
