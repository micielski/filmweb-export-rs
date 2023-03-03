use clap::Parser;
use colored::Colorize;
use flume::Sender;
use lazy_static::lazy_static;
use std::fmt::Display;
use std::io::{stdin, stdout, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use filmed::filmweb::auth::{ExportFiles, FilmwebUser, RatedPage, UserPage, UserPageType};
use filmed::imdb::IMDb;
use filmed::{IMDbLookup, RatedTitle, Title, TitleID, User};

#[derive(Parser, Debug)]
#[command(name = "filmweb-export")]
#[command(author = "Remigiusz Micielski <remigiusz.micielski@gmail.com>")]
#[command(version = "0.2.2")]
#[command(about = "Exports user data from filmweb.pl to IMDBv3 csv file format", long_about = None)]
struct Args {
    #[arg(short, long, value_parser)]
    username: Option<String>,

    /// _fwuser_token cookie value
    #[arg(short, long, value_parser)]
    token: Option<String>,

    /// _fwuser_sessionId cookie value
    #[arg(short, long, value_parser)]
    session: Option<String>,

    /// JWT cookie value
    #[arg(short, long, value_parser)]
    jwt: Option<String>,

    /// Number of threads to spawn
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..8))]
    threads: u8,

    /// If enabled, successfully exported titles won't be printed
    #[arg(short, long, value_parser, default_value_t = false)]
    quiet: bool,
}

lazy_static! {
    static ref ARGS: Args = Args::parse();
}

fn main() {
    println!("{}", "filmweb-export starting...".yellow());
    check_is_filmweb_reachable();
    env_logger::init();

    let mut export_files = ExportFiles::default();
    let (token, session, jwt) = handle_empty_credentials(&ARGS);
    let user = FilmwebUser::new(token, session, jwt).expect("credentials ok");

    // Get count of rated films, and convert it to number of pages
    let movies_pages = user.num_of_rated_movies() / 25 + 1;
    let shows_pages = user.num_of_rated_shows() / 25 + 1;
    let watchlist_pages = user.num_of_watchlisted_titles() / 25 + 1;
    let total_pages = movies_pages + shows_pages + watchlist_pages;

    let exported_pages: Arc<Mutex<Vec<RatedPage>>> = Arc::new(Mutex::new(Vec::with_capacity(total_pages as usize)));

    let imdb_client = Arc::new(IMDb::new());
    let (handle, tx) = imdb_scraping_thread(&Arc::clone(&exported_pages), total_pages, imdb_client);

    // Scraping actual data from Filmweb
    for (pages_count, page_type) in [
        (movies_pages, UserPageType::RatedFilms),
        (shows_pages, UserPageType::RatedShows),
        (watchlist_pages, UserPageType::Watchlist),
    ] {
        scrape_fw(pages_count, &user, page_type, &Arc::clone(&tx)).unwrap();
    }

    handle.join().unwrap();

    // Check for possible false errors (in duration comparison only for now), and let the user
    // decide if it's a good match
    for page in &mut *exported_pages.lock().unwrap() {
        for title in &mut *page.rated_titles {
            if title.imdb_data().is_some() {
                let imdb_duration = title
                    .imdb_data()
                    .unwrap()
                    .duration()
                    .expect("imdb always have a duration");
                let imdb_year = title.imdb_data().unwrap().year();
                // TODO: move title.is_year_similar check to library
                if title.is_duration_similar(u32::from(imdb_duration)) && title.is_year_similar(imdb_year) {
                    title.to_csv_imdbv3_tmdb_files(&mut export_files);
                } else {
                    let url = format!(
                        "https://www.imdb.com/title/{}",
                        title.imdb_data().expect("has imdb_data").id
                    );
                    let question = format!("{} Is {url} a good match for {}? (y/N): ", "[?]".blue(), title.title());
                    if user_agrees(question) {
                        title.to_csv_imdbv3_tmdb_files(&mut export_files);
                    } else {
                        // Replace the title's imdb_data field Some(imdb_data) with None so it's marked
                        // as not found at IMDb
                        drop(title.imdb_data_owned());
                    }
                }
            }
        }
    }
    print_failed(&Arc::clone(&exported_pages));
}

fn handle_empty_credentials(args: &ARGS) -> (String, String, String) {
    let ask_for_cookie = |cookie_name: &'static str| -> String {
        print!("{} {cookie_name} cookie value: ", "[?]".blue());
        stdout().flush().expect("term ok");
        let mut cookie = String::new();
        stdin().read_line(&mut cookie).expect("term ok");
        cookie.trim().to_string()
    };

    let token = {
        if let Some(ref token) = args.token {
            token.clone()
        } else {
            ask_for_cookie("_fwuser_token")
        }
    };

    let session = {
        if let Some(ref session) = args.session {
            session.clone()
        } else {
            ask_for_cookie("_fwuser_sessionId")
        }
    };

    let jwt = {
        if let Some(ref jwt) = args.jwt {
            jwt.clone()
        } else {
            ask_for_cookie("JWT")
        }
    };

    (token, session, jwt)
}

fn scrape_fw(
    total_pages: u16,
    user: &FilmwebUser,
    titles_type: UserPageType,
    tx: &Mutex<Sender<RatedPage>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // just to print out what is being scraped
    let what = match titles_type {
        UserPageType::RatedFilms => "films",
        UserPageType::RatedShows => "serials",
        UserPageType::Watchlist => "wants2see",
    };
    let page_type = Arc::new(&titles_type);
    let error_happened = Arc::new(AtomicBool::new(false));
    let scraped_pages_count = Arc::new(AtomicUsize::new(0));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(ARGS.threads as usize)
        .build()?;
    pool.scope(|s| {
        for i in 1..=total_pages {
            let scraped_pages_count = scraped_pages_count.clone();
            let page_type = Arc::clone(&page_type);
            let error_happened = Arc::clone(&error_happened);
            s.spawn(move |_| {
                let page_type = match *page_type {
                    UserPageType::RatedFilms => UserPage::RatedFilms(i as u8),
                    UserPageType::RatedShows => UserPage::RatedShows(i as u8),
                    UserPageType::Watchlist => UserPage::Watchlist(i as u8),
                };
                let mut fw_page = user.scrape(page_type);
                if let Err(e) = fw_page.as_mut() {
                    eprintln!("{} {e}", "error occured:".red());
                    error_happened.store(false, Ordering::Relaxed);
                    std::process::exit(1);
                };
                tx.lock().unwrap().send(fw_page.unwrap()).unwrap();
                scraped_pages_count.store(scraped_pages_count.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
                println!(
                    "{} Scraping {what}... [{}/{total_pages}]",
                    "[i]".blue(),
                    scraped_pages_count.load(Ordering::Relaxed)
                );
                stdout().flush().expect("term ok");
            });
        }
    });
    // Check if any of spawned threads returned an error
    if error_happened.load(Ordering::SeqCst) {
        eprintln!("{}", "Exiting due to some thread(s) reporting error(s)".red());
        std::process::exit(1);
    }
    Ok(())
}

fn user_agrees(question: impl Display) -> bool {
    loop {
        print!("{question}");
        std::io::stdout().flush().expect("term ok");
        let mut decision = String::new();
        stdin().read_line(&mut decision).expect("term ok");
        decision = decision.trim().to_lowercase();
        if decision == "y" || decision == "yes" {
            return true;
        } else if decision == "n" || decision == "no" || decision.is_empty() {
            return false;
        }
        println!("{} Not understood", "[?]".yellow());
    }
}

fn imdb_scraping_thread(
    exported_pages: &Arc<Mutex<Vec<RatedPage>>>,
    pages_count: u16,
    imdb_client: Arc<IMDb>,
) -> (JoinHandle<()>, Arc<Mutex<Sender<RatedPage>>>) {
    let (tx, rx) = flume::unbounded::<RatedPage>();
    let rx = Arc::new(Mutex::new(rx));
    let tx = Arc::new(Mutex::new(tx));
    let exported_pages_clone = Arc::clone(exported_pages);
    let handle = thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(ARGS.threads as usize)
            .build()
            .unwrap();
        pool.scope(|s| {
            for _ in 0..pages_count {
                let mut page = rx.lock().unwrap().recv().unwrap();
                let exported_pages_clone = Arc::clone(&exported_pages_clone);
                let imdb_client_clone = Arc::clone(&imdb_client);
                s.spawn(move |_| {
                    for title in &mut page.rated_titles {
                        title.set_imdb_data_with_lookup(&imdb_client_clone).ok();
                        if !ARGS.quiet {
                            print_title(title);
                        }
                    }
                    exported_pages_clone.lock().unwrap().push(page);
                });
            }
        });
    });
    (handle, tx)
}

fn print_failed(pages: &Arc<Mutex<Vec<RatedPage>>>) {
    println!("Following titles couldn't be found:");
    for page in &*pages.lock().unwrap() {
        for title in &page.rated_titles {
            if title.imdb_data().is_none() {
                print_title(title);
            }
        }
    }
}

fn print_title<T: RatedTitle + IMDbLookup>(fw_title: &T) {
    let print_rating = || {
        if fw_title.is_favorited() {
            format!(
                "{}/10 \u{2665}",
                fw_title.rating().expect("It's favorited so it's rated")
            )
            .red()
        } else if fw_title.rating().is_some() {
            format!("{}/10", fw_title.rating().expect("It's some")).normal()
        } else {
            String::new().normal()
        }
    };

    let print_found = |imdb_id: &TitleID, imdb_name: &str| {
        let add_prefix = "[+]".green();
        let title_name = fw_title.title();
        let title_year = fw_title.year();
        let rating = print_rating();
        let separator = "|".dimmed();
        let imdb_name = imdb_name.dimmed();
        let imdb_title_url = format!("{}{}", "https://imdb.com/title/".dimmed(), imdb_id.to_string().dimmed());
        println!("{add_prefix} {title_name} {title_year} {rating} {separator} {imdb_name} {imdb_title_url}");
    };

    let print_not_found = || {
        println!("{} {} {}", "[-]".red(), fw_title.title(), print_rating());
    };

    fw_title.imdb_data().map_or_else(print_not_found, |imdb_data| {
        print_found(&imdb_data.id, imdb_data.title());
    });
}

fn check_is_filmweb_reachable() {
    let prefix = "[!]".red();
    match reqwest::blocking::get("https://www.filmweb.pl") {
        Ok(res) => {
            if res.status().is_success() {
                return;
            } else if res.status().is_server_error() {
                println!(
                    "{prefix} Filmweb's servers are experiencing some issues, try again later. Status: {:?}",
                    res.status()
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            println!("{prefix} Couldn't connect to Filmweb. Error: {:?}", e);
            std::process::exit(1);
        }
    };
}
