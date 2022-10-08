use clap::Parser;
use colored::Colorize;
use reqwest::blocking::Client;
use std::{
    error::Error,
    io,
    io::Write,
    sync::{Arc, Mutex},
    thread,
    time,
};

use filmweb_export_rs::{
    filmweb_client_builder, imdb_client_builder, ExportFiles, FwPage, FwPageNumber, FwRatedTitle, FwUser,
    IMDbApiDetails, FwTitleType
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
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
    #[arg(long, default_value_t = 7)]
    threads: u8,

    /// Delay in seconds between IMDb requests
    #[arg(long, default_value_t = 6)]
    delay: u8,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    println!("{}", "filmweb-export starting...".yellow());

    let exported_pages: Arc<Arc<Mutex<Vec<FwPage>>>> = Arc::new(Arc::new(Mutex::new(Vec::new())));
    let export_files = Arc::new(Mutex::new(ExportFiles::default()));
    let user = FwUser::new(args.username, args.token, args.session, args.jwt);

    let fw_client = filmweb_client_builder(&user).unwrap();
    let imdb_client = imdb_client_builder().unwrap();

    // Get count of rated films, and convert it to number of pages
    let counts = user.get_counts(&fw_client)?;
    let films_pages = (counts.0 / 25 + 1) as u8;
    let serials_pages = (counts.1 / 25 + 1) as u8;
    let wants2see_pages = (counts.2 / 25 + 1) as u8;

    // Scraping actual data from Filmweb
    for (what, pages_count, page_type) in [
        ("films", films_pages, FwTitleType::Film),
        ("serials", serials_pages, FwTitleType::Serial),
        ("wants2see", wants2see_pages, FwTitleType::WantsToSee)
    ] {
        scrape_fw(
            pages_count,
            &user,
            page_type,
            what,
            &fw_client,
            &Arc::clone(&exported_pages),
            args.threads
        );
    }

    get_imdb_data_and_save(&Arc::clone(&exported_pages), &imdb_client, &export_files, args.delay);
    print_failed(&Arc::clone(&exported_pages));

    Ok(())
}

fn scrape_fw(
    total_pages: u8,
    user: &FwUser,
    page_type: FwTitleType,
    what: &str,
    fw_client: &Client,
    pages: &Arc<Mutex<Vec<FwPage>>>,
    threads: u8,
) {
    let page_type_arc = Arc::new(&page_type);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads as usize).build().unwrap();
    pool.scope(|s| {
        for i in 1..=total_pages {
            let page_type_clone = Arc::clone(&page_type_arc);
            let pages_clone = Arc::clone(pages);
            s.spawn(move |_| {
                let page_type = match *page_type_clone {
                    FwTitleType::Film => FwPageNumber::Films(i),
                    FwTitleType::Serial => FwPageNumber::Serials(i),
                    FwTitleType::WantsToSee => FwPageNumber::WantsToSee(i),
                };
                let mut fw_page = FwPage::new(page_type, user, fw_client);
                if fw_page.scrape_from_page(fw_client).is_err() {
                    eprintln!("Error occured");
                    std::process::exit(1);
                };
                pages_clone.lock().unwrap().push(fw_page);
                print!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
                io::stdout().flush().unwrap();
            });
        }
    });
    println!();
}

fn get_imdb_data_and_save(
    pages: &Arc<Mutex<Vec<FwPage>>>,
    // fw_client: &Client,
    imdb_client: &Client,
    export_files: &Arc<Mutex<ExportFiles>>,
    delay: u8,
) {
    let mut pages_iter = pages.lock().unwrap();
    thread::scope(|s| {
        for page in pages_iter.iter_mut() {
            let export_files_clone = Arc::clone(export_files);
            s.spawn(move || {
                for title in &mut page.rated_titles {
                    title.get_imdb_data_logic(imdb_client);
                    if !title.is_duration_ok() {
                        title.imdb_data = None;
                    }
                    print_title(title);
                    title.export_csv(&mut export_files_clone.lock().unwrap());
                }
            });
            thread::sleep(time::Duration::from_secs(delay as u64));
        }
    });
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
            "-> | ".dimmed(),
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
