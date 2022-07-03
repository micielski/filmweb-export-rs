use clap::Parser;
use colored::{ColoredString, Colorize};
use reqwest::Client;
use std::{error::Error, io, io::Write};

use filmweb_export_rs::*;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_parser)]
    username: String,

    #[clap(short, long, value_parser)]
    token: String,

    #[clap(short, long, value_parser)]
    session: String,

    #[clap(short, long, value_parser)]
    jwt: String,
}

struct Pages {
    films: Vec<FwPage>,
    serials: Vec<FwPage>,
    wants2see: Vec<FwPage>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    export(args).await?;
    Ok(())
}

async fn export(args: Args) -> Result<(), Box<dyn Error>> {
    let mut export_files = ExportFiles::default();
    let user = FwUser::new(args.username, args.token, args.session, args.jwt).await;
    let fw_client = filmweb_client_builder(&user).unwrap();
    let counts = user.get_counts(&fw_client).await?;
    let imdb_client = imdb_client_builder().unwrap();

    let mut pages = Pages {
        films: Vec::new(),
        serials: Vec::new(),
        wants2see: Vec::new(),
    };

    // AWAIT IS ONLY ALLOWED INSIDE ASYNC FUNCTIONS AND BLOCKS AWAIT IS ONLY ALLOWED INSIDE ASYNC FUNCTIONS AND BLOCKS AWAIT IS ONLY ALLOWED INSIDE ASYNC FUNCTIONS AND BLOCKS
    // (1..=counts.0/25+1).into_iter().map(|i| fetch_page(&user, i, FwPageType::Films, &fw_client, &mut pages).await);
    let films_pages = (counts.0 as f64 / 25_f64 + 1_f64.ceil()) as u16;
    let serials_pages = (counts.1 as f64 / 25_f64 + 1_f64.ceil()) as u16;
    let wants2see_pages = (counts.2 as f64 / 25_f64 + 1_f64.ceil()) as u16;
    print!("\r{} Scraping films...", "[i]".blue());
    for i in 1..=films_pages {
        fetch_page(&user, i, FwPageType::Films, &fw_client, &mut pages).await;
        print!("\r{} Scraping films... [{}/{}]", "[i]".blue(), i, films_pages);
        io::stdout().flush().unwrap();
    }

    print!("\r{} Scraping serials...", "[i]".blue());
    for i in 1..=serials_pages {
        fetch_page(&user, i, FwPageType::Serials, &fw_client, &mut pages).await;
        print!("\r{} Scraping serials... [{}/{}]", "[i]".blue(), i, serials_pages);
        io::stdout().flush().unwrap();
    }

    print!("\r{} Scraping wants2see...", "[i]".blue());
    for i in 1..=wants2see_pages {
        fetch_page(&user, i, FwPageType::WantsToSee, &fw_client, &mut pages).await;
        if i != wants2see_pages {
            print!("\r{} Scraping wants2see... [{}/{}]", "[i]".blue(), i, wants2see_pages);
        } else {
            println!("\r{} Scraping wants2see... [{}/{}]", "[i]".blue(), i, wants2see_pages);
        }
        io::stdout().flush().unwrap();
    }

    imdb_id_and_export(pages.films, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.serials, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.wants2see, &imdb_client, &mut export_files).await;

    Ok(())
}

async fn fetch_page(user: &FwUser, page: u16, page_type: FwPageType, fw_client: &Client, pages: &mut Pages) {
    let mut fw_page = FwPage::new(page as u8, page_type, user, fw_client).await;
    fw_page.scrape_voteboxes(fw_client).await.unwrap();
    pages.films.push(fw_page);
}

async fn imdb_id_and_export(pages: Vec<FwPage>, imdb_client: &Client, export_files: &mut ExportFiles) {
    for page in pages {
        for mut title in page.rated_titles {
            title.get_imdb_ids_logic(imdb_client).await;
            print_title(&title);
            title.export_csv(export_files);
        }
    }
}

fn print_title(title: &FwRatedTitle) {
    match &title.imdb_id {
        Some(id) => println!(
            "{} {} {} {}{}",
            "[+]".green(),
            title.title_pl,
            print_rating(&title.rating.as_ref()),
            "|".dimmed(),
            id.dimmed()
        ),
        None => println!(
            "{} {} {}",
            "[-]".red(),
            title.title_pl,
            print_rating(&title.rating.as_ref())
        ),
    }
}

// should i make it a closure
fn print_rating(fw_api: &Option<&FwApiDetails>) -> ColoredString {
    match fw_api {
        Some(api) => match api.favorite {
            true => format!("{}/10 ♥", api.rate).red(),
            false => format!("{}/10", api.rate).normal(),
        },
        None => "".to_string().normal(),
    }
}
