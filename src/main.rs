use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use lazy_static::lazy_static;
use reqwest::blocking::Client;
use std::io::{stdin, stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use filmweb_export_rs::{
    imdb_client_builder, ExportFiles, FwErrors, FwPage, FwPageNumber, FwRatedTitle, FwTitleType, FwUser, IMDbApiDetails,
};

#[derive(Parser, Debug)]
#[command(name = "filmweb-export")]
#[command(author = "Remigiusz M <remigiusz.micielski@gmail.com>")]
#[command(version = "0.1.0")]
#[command(about = "Exports user data from filmweb.pl to IMDBv2 csv file format", long_about = None)]
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
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u8).range(1..7))]
    threads: u8,

    /// If enabled, successfully exported titles won't be printed
    #[arg(short, long, value_parser, default_value_t = false)]
    quiet: bool,
}

struct ClientPool {
    clients: Vec<Client>,
}

impl ClientPool {
    fn get_a_client(&self, i: u8) -> &Client {
        &self.clients[(i % self.clients.len() as u8) as usize]
    }

    // TODO: consume client_sample
    fn new(client_sample: &Client, amount: u8) -> Self {
        let mut clients = Vec::new();
        for _ in 0..amount {
            clients.push(client_sample.clone());
        }

        Self { clients }
    }
}

lazy_static! {
    static ref ARGS: Args = Args::parse();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "filmweb-export starting...".yellow());

    let mut export_files = ExportFiles::default();
    let (token, session, jwt) = handle_empty_credentials(&ARGS);
    let fw_client = FwUser::filmweb_client_builder(&token, &session, &jwt)?;
    let username = handle_empty_username(&ARGS, &fw_client);
    let mut user = FwUser::new(username, token, session, jwt);
    let fw_client_pool = ClientPool::new(&fw_client, 9);

    let imdb_client = imdb_client_builder()?;
    let imdb_client_pool = Arc::new(ClientPool::new(&imdb_client, 9));

    // Get count of rated films, and convert it to number of pages
    user.get_counts(fw_client_pool.get_a_client(1))?;
    let titles_counts = user.titles_count.unwrap(); // if above line executed, it won't panic
    let films_pages = (titles_counts.films / 25 + 1) as u8;
    let serials_pages = (titles_counts.serials / 25 + 1) as u8;
    let wants2see_pages = (titles_counts.marked_to_see / 25 + 1) as u8;
    let total_pages = films_pages + serials_pages + wants2see_pages;
    let exported_pages: Arc<Mutex<Vec<FwPage>>> = Arc::new(Mutex::new(Vec::with_capacity(total_pages as usize)));

    let (handle, tx) = imdb_scraping_thread(&Arc::clone(&exported_pages), imdb_client_pool, total_pages);

    // Scraping actual data from Filmweb
    for (pages_count, page_type) in [
        (films_pages, FwTitleType::Film),
        (serials_pages, FwTitleType::Serial),
        (wants2see_pages, FwTitleType::WantsToSee),
    ] {
        scrape_fw(pages_count, &user, page_type, &fw_client_pool, &Arc::clone(&tx)).unwrap();
    }

    handle.join().unwrap();

    // Check for possible false errors (in duration comparison only for now), and let the user
    // decide if it's a good match
    for page in &mut *exported_pages.lock().unwrap() {
        for title in &mut *page.rated_titles {
            if let Some(ref imdb_data) = title.imdb_data {
                if title.is_duration_ok() {
                    title.export_csv(&mut export_files);
                } else {
                    let url = format!("https://www.imdb.com/title/tt{}", imdb_data.id);
                    print!(
                        "{} Is {} a good match for {}? (y/N): ",
                        "[?]".blue(),
                        url,
                        title.fw_title_pl
                    );
                    std::io::stdout().flush()?;
                    let mut decision = String::new();
                    stdin().read_line(&mut decision).expect("Invalid input");
                    println!();
                    if decision.trim().to_lowercase() == "y" {
                        title.export_csv(&mut export_files);
                    } else {
                        title.imdb_data = None;
                    }
                }
            }
        }
    }
    print_failed(&Arc::clone(&exported_pages));

    Ok(())
}

fn handle_empty_credentials(args: &ARGS) -> (String, String, String) {
    let ask_for_cookie = |cookie_name: &'static str| -> String {
        print!("{} {} cookie value: ", "[?]".blue(), cookie_name);
        stdout().flush().unwrap();
        let mut cookie = String::new();
        stdin().read_line(&mut cookie).unwrap();
        cookie
    };

    let token = if args.token.is_none() {
        ask_for_cookie("_fwuser_token")
    } else {
        args.token.as_ref().unwrap().clone()
    };
    let session = if args.session.is_none() {
        ask_for_cookie("_fwuser_sessionId")
    } else {
        args.session.as_ref().unwrap().clone()
    };
    let jwt = if args.jwt.is_none() {
        ask_for_cookie("JWT")
    } else {
        args.jwt.as_ref().unwrap().clone()
    };
    (token, session, jwt)
}

fn handle_empty_username(args: &ARGS, fw_client: &Client) -> String {
    if args.username.is_none() {
        FwUser::get_username(fw_client).unwrap()
    } else {
        args.username.as_ref().unwrap().clone()
    }
}

fn scrape_fw(
    total_pages: u8,
    user: &FwUser,
    titles_type: FwTitleType,
    clientpool: &ClientPool,
    tx: &Mutex<Sender<FwPage>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // just to print out what is being scraped
    let what = match titles_type {
        FwTitleType::Film => "films",
        FwTitleType::Serial => "serials",
        FwTitleType::WantsToSee => "wants2see",
    };

    let page_type_arc = Arc::new(&titles_type);
    let error_happened = Arc::new(AtomicBool::new(false));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(ARGS.threads as usize)
        .build()?;
    pool.scope(|s| {
        for i in 1..=total_pages {
            let page_type_clone = Arc::clone(&page_type_arc);
            let error_happened_clone = Arc::clone(&error_happened);
            s.spawn(move |_| {
                let page_type = match **page_type_clone {
                    FwTitleType::Film => FwPageNumber::Films(i),
                    FwTitleType::Serial => FwPageNumber::Serials(i),
                    FwTitleType::WantsToSee => FwPageNumber::WantsToSee(i),
                };
                let mut fw_page = FwPage::new(page_type, user, clientpool.get_a_client(i));
                if let Err(e) = fw_page.scrape_from_page(clientpool.get_a_client(i)) {
                    match e {
                        FwErrors::InvalidJwt => {
                            eprintln!(
                                "{}",
                                "JWT is invalid/has invalidated. Try again with a fresh cookie".red()
                            );
                            // *error_happened_clone.lock().unwrap() = true;
                            error_happened_clone.store(false, Ordering::Relaxed);
                            std::process::exit(1)
                        }
                        FwErrors::InvalidYear { title_id, failed_year } => {
                            eprintln!("{} {}", "Couldn't parse a year for title with id".red(), title_id);
                            eprintln!("{} {}", "String that failed to parse:".blue(), failed_year);
                            error_happened_clone.store(true, Ordering::Relaxed);
                            std::process::exit(1)
                        }
                        FwErrors::ZeroResults | FwErrors::InvalidDuration | FwErrors::InvalidCredentials => {
                            unreachable!()
                        }
                    }
                };
                tx.lock().unwrap().send(fw_page).unwrap();
                println!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
                stdout().flush().unwrap();
            });
        }
    });
    println!();
    // Check if any of spawned threads returned an error
    if error_happened.load(Ordering::SeqCst) {
        eprintln!("{}", "Exiting due to some thread(s) reporting error(s)".red());
        std::process::exit(1);
    }
    Ok(())
}

fn imdb_scraping_thread(
    exported_pages: &Arc<Mutex<Vec<FwPage>>>,
    imdb_client_pool: Arc<ClientPool>,
    pages_count: u8,
) -> (JoinHandle<()>, Arc<Mutex<Sender<FwPage>>>) {
    let (tx, rx) = channel::<FwPage>();
    let rx = Arc::new(Mutex::new(rx));
    let tx = Arc::new(Mutex::new(tx));
    let exported_pages_clone = Arc::clone(exported_pages);
    let handle = thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(ARGS.threads as usize)
            .build()
            .unwrap();
        pool.scope(|s| {
            for i in 0..pages_count {
                let mut page = rx.lock().unwrap().recv().unwrap();
                let imdb_client_pool_clone = Arc::clone(&imdb_client_pool);
                let exported_pages_clone = Arc::clone(&exported_pages_clone);
                s.spawn(move |_| {
                    for title in &mut page.rated_titles {
                        title.get_imdb_data_logic(imdb_client_pool_clone.get_a_client(i as u8));
                        print_title(title);
                    }
                    exported_pages_clone.lock().unwrap().push(page);
                });
            }
        });
    });
    (handle, tx)
}

fn print_failed(pages: &Arc<Mutex<Vec<FwPage>>>) {
    println!("Following titles couldn't be found:");
    for page in &*pages.lock().unwrap() {
        for title in &page.rated_titles {
            if title.imdb_data.is_none() {
                print_title(title);
            }
        }
    }
}

fn print_title(title: &FwRatedTitle) {
    // Prints a rating with, or without a heart (if a title is favorited or not)
    let print_rating = || match title.rating.as_ref() {
        Some(api) if api.favorite => format!("{}/10 \u{2665}", api.rate).red(),
        Some(api) => format!("{}/10", api.rate).normal(),
        _ => "".to_owned().normal(),
    };

    let print_not_found = || {
        println!("{} {} {}", "[-]".red(), title.fw_title_pl, print_rating());
    };

    let imdb_title = match title.imdb_data {
        Some(ref data) => &data.title,
        None => unreachable!(),
    };

    let print_found = |imdb_api: &IMDbApiDetails| {
        println!(
            "{} {} {} {}{} {}",
            "[+]".green(),
            title.fw_title_pl,
            print_rating(),
            "| ".dimmed(),
            imdb_title.dimmed(),
            imdb_api.id.dimmed()
        );
    };

    match title.imdb_data {
        Some(ref data) if data.id != "not-found" && !ARGS.quiet => print_found(data),
        Some(ref data) if data.id != "not-found" && ARGS.quiet => (),
        Some(ref data) if data.id == "not-found" => print_not_found(),
        _ => print_not_found(),
    }
}
