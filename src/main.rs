use anyhow::Result;
use clap::Parser;
use colored::Colorize;
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
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u8).range(1..8))]
    threads: u8,
}

fn main() -> Result<()> {
    let args = Args::parse();
    println!("{}", "filmweb-export starting...".yellow());

    let mut export_files = ExportFiles::default();
    let mut user = FwUser::new(args.username, args.token, args.session, args.jwt);
    let fw_client = user.filmweb_client_builder()?;

    let imdb_client = imdb_client_builder()?;

    // Get count of rated films, and convert it to number of pages
    user.get_counts(&fw_client).unwrap();
    let titles_counts = user.titles_count.unwrap(); // if above line executed, it won't panic
    let films_pages = (titles_counts.films / 25 + 1) as u8;
    let serials_pages = (titles_counts.serials / 25 + 1) as u8;
    let wants2see_pages = (titles_counts.marked_to_see / 25 + 1) as u8;
    let exported_pages: Arc<Arc<Mutex<Vec<FwPage>>>> = Arc::new(Arc::new(Mutex::new(Vec::with_capacity(
        (films_pages + serials_pages + wants2see_pages) as usize,
    ))));

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
            &fw_client,
            &Arc::clone(&exported_pages),
            args.threads,
        )
        .unwrap();
    }

    fetch_imdb_data(&Arc::clone(&exported_pages), &imdb_client, args.threads);

    // Check for possible false errors (in duration comparison only for now), and let the user
    // decide if it's a good match
    for page in &mut *exported_pages.lock().unwrap() {
        for title in &mut *page.rated_titles {
            if let Some(ref imdb_data) = title.imdb_data {
                if !title.is_duration_ok() {
                    let url = format!("https://www.imdb.com/title/tt{}", imdb_data.id);
                    print!("Is {} a good match for {}? (y/n): ", url, title.fw_title_pl);
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
    fw_client: &Client,
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
                let mut fw_page = FwPage::new(page_type, user, fw_client);
                if let Err(e) = fw_page.scrape_from_page(fw_client) {
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
                            std::process::exit(1)
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

fn fetch_imdb_data(pages: &Arc<Mutex<Vec<FwPage>>>, imdb_client: &Client, threads: u8) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads as usize)
        .build()
        .unwrap();
    let pages_iter = &mut *pages.lock().unwrap();
    pool.scope(|s| {
        for page in &mut *pages_iter {
            s.spawn(move |_| {
                for title in &mut page.rated_titles {
                    title.get_imdb_data_logic(imdb_client);
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
    let print_rating = || match &title.rating.as_ref() {
        Some(api) => {
            if api.favorite {
                format!("{}/10 ♥", api.rate).red()
            } else {
                format!("{}/10", api.rate).normal()
            }
        }
        None => "".to_string().normal(),
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

    match &title.imdb_data {
        Some(data) => match data.id.as_str() {
            "not-found" => print_not_found(),
            _ => print_found(data),
        },
        None => print_not_found(),
    }
}
