use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use lazy_static::lazy_static;
use reqwest::blocking::Client;
use std::io::stdin;
use std::sync::{Arc, Mutex};
use std::{io, io::Write};

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
    username: String,

    /// _fwuser_token cookie value
    #[arg(short, long, value_parser)]
    token: String,

    /// _fwuser_sessionId cookie value
    #[arg(short, long, value_parser)]
    session: String,

    /// JWT cookie value
    #[arg(short, long, value_parser)]
    jwt: String,

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

    fn new(client_sample: Client, amount: u8) -> ClientPool {
        let mut clients = Vec::new();
        for _ in 0..amount {
            clients.push(client_sample.clone())
        }

        ClientPool { clients }
    }
}

lazy_static! {
    static ref ARGS: Args = Args::parse();
}

fn main() -> Result<()> {
    // let ARGS = Args::parse();
    println!("{}", ARGS.quiet);
    println!("{}", "filmweb-export starting...".yellow());

    let mut export_files = ExportFiles::default();
    let mut user = FwUser::new(
        ARGS.username.clone(),
        ARGS.token.clone(),
        ARGS.session.clone(),
        ARGS.jwt.clone(),
    );
    let fw_client = user.filmweb_client_builder()?;
    let fw_client_pool = ClientPool::new(fw_client, 3);

    let imdb_client = imdb_client_builder()?;
    let imdb_client_pool = ClientPool::new(imdb_client, 3);

    // Get count of rated films, and convert it to number of pages
    user.get_counts(fw_client_pool.get_a_client(1)).unwrap();
    let titles_counts = user.titles_count.unwrap(); // if above line executed, it won't panic
    let films_pages = (titles_counts.films / 25 + 1) as u8;
    let serials_pages = (titles_counts.serials / 25 + 1) as u8;
    let wants2see_pages = (titles_counts.marked_to_see / 25 + 1) as u8;
    let exported_pages: Arc<Mutex<Vec<FwPage>>> = Arc::new(Mutex::new(Vec::with_capacity(
        (films_pages + serials_pages + wants2see_pages) as usize,
    )));

    // Scraping actual data from Filmweb
    for (pages_count, page_type) in [
        (films_pages, FwTitleType::Film),
        (serials_pages, FwTitleType::Serial),
        (wants2see_pages, FwTitleType::WantsToSee),
    ] {
        scrape_fw(
            pages_count,
            &user,
            page_type,
            &fw_client_pool,
            &Arc::clone(&exported_pages),
            ARGS.threads,
        )
        .unwrap();
    }

    fetch_imdb_data(&Arc::clone(&exported_pages), &imdb_client_pool, ARGS.threads);

    // Check for possible false errors (in duration comparison only for now), and let the user
    // decide if it's a good match
    for page in &mut *exported_pages.lock().unwrap() {
        for title in &mut *page.rated_titles {
            if let Some(ref imdb_data) = title.imdb_data {
                if !title.is_duration_ok() {
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
                        title.export_csv(&mut export_files)
                    } else {
                        title.imdb_data = None;
                    }
                } else {
                    title.export_csv(&mut export_files)
                }
            }
        }
    }
    print_failed(&Arc::clone(&exported_pages));

    Ok(())
}

fn scrape_fw(
    total_pages: u8,
    user: &FwUser,
    page_type: FwTitleType,
    clientpool: &ClientPool,
    pages: &Arc<Mutex<Vec<FwPage>>>,
    threads: u8,
) -> Result<()> {
    // just to print out what is being scraped
    let what = match page_type {
        FwTitleType::Film => "films",
        FwTitleType::Serial => "serials",
        FwTitleType::WantsToSee => "wants2see",
    };

    let page_type_arc = Arc::new(&page_type);
    let error_happened = Arc::new(Mutex::new(false));
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads as usize).build()?;
    pool.scope(|s| {
        for i in 1..=total_pages {
            let page_type_clone = Arc::clone(&page_type_arc);
            let error_happened_clone = Arc::clone(&error_happened);
            let pages_clone = Arc::clone(pages);
            s.spawn(move |_| {
                let page_type = match *page_type_clone {
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
                            *error_happened_clone.lock().unwrap() = true;
                            std::process::exit(1)
                        }
                        FwErrors::InvalidYear { title_id, failed_year } => {
                            eprintln!("{} {}", "Couldn't parse a year for title with id".red(), title_id);
                            eprintln!("{} {}", "String that failed to parse:".blue(), failed_year);
                            *error_happened_clone.lock().unwrap() = true;
                            *error_happened_clone.lock().unwrap() = true;
                        }
                        _ => unreachable!(),
                    }
                };
                pages_clone.lock().unwrap().push(fw_page);
                print!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
                io::stdout().flush().unwrap();
            });
        }
    });
    println!();
    // Check if any of spawned threads returned an error
    if *error_happened.lock().unwrap() {
        eprintln!("{}", "Exiting due to some thread(s) reporting error(s)".red());
        std::process::exit(1);
    }
    Ok(())
}

fn fetch_imdb_data(pages: &Arc<Mutex<Vec<FwPage>>>, imdb_client: &ClientPool, threads: u8) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads as usize)
        .build()
        .unwrap();
    let pages_iter = &mut *pages.lock().unwrap();
    pool.scope(|s| {
        for (i, page) in pages_iter.iter_mut().enumerate() {
            s.spawn(move |_| {
                for title in &mut page.rated_titles {
                    title.get_imdb_data_logic(imdb_client.get_a_client(i as u8));
                    print_title(title);
                }
            })
        }
    })
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
    // let print_rating = || match &title.rating.as_ref() {
    //     Some(api) => {
    //         if api.favorite {
    //             format!("{}/10 ♥", api.rate).red()
    //         } else {
    //             format!("{}/10", api.rate).normal()
    //         }
    //     }
    //     None => "".to_string().normal(),
    // };
    let print_rating = || match &title.rating.as_ref() {
        Some(api) if api.favorite => format!("{}/10 ♥", api.rate).red(),
        Some(api) => format!("{}/10", api.rate).normal(),
        _ => "".to_string().normal(),
    };

    let print_not_found = || {
        println!("{} {} {}", "[-]".red(), title.fw_title_pl, print_rating());
    };

    let print_found = |imdb_api: &IMDbApiDetails| {
        println!(
            "{} {} {} {}{} {}",
            "[+]".green(),
            title.fw_title_pl,
            print_rating(),
            "| ".dimmed(),
            title.imdb_data.as_ref().unwrap().title.dimmed(),
            imdb_api.id.dimmed()
        );
    };

    match title.imdb_data {
        Some(ref data) if data.id.as_str() != "not-found" && !ARGS.quiet => print_found(&data),
        Some(ref data) if data.id.as_str() != "not-found" && ARGS.quiet => (),
        Some(ref data) if data.id.as_str() == "not-found" => print_not_found(),
        _ => print_not_found(),
    }
}
