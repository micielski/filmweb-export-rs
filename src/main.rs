use clap::Parser;
use colored::{Colorize};
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

    let films_pages = (counts.0 as f64 / 25_f64 + 1_f64.ceil()) as u16;
    let serials_pages = (counts.1 as f64 / 25_f64 + 1_f64.ceil()) as u16;
    let wants2see_pages = (counts.2 as f64 / 25_f64 + 1_f64.ceil()) as u16;

    print!("\r{} Scraping films...", "[i]".blue());
    scrape_fw(films_pages, &user, FwPageType::Films, "films",  &fw_client, &mut pages).await;

    print!("\r{} Scraping serials...", "[i]".blue());
    scrape_fw(serials_pages, &user, FwPageType::Serials, "serials",  &fw_client, &mut pages).await;

    print!("\r{} Scraping wants2see...", "[i]".blue());
    scrape_fw(wants2see_pages, &user, FwPageType::WantsToSee, "wants2see",  &fw_client, &mut pages).await;

    imdb_id_and_export(pages.films, &fw_client, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.serials, &fw_client, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.wants2see, &fw_client, &imdb_client, &mut export_files).await;

    Ok(())
}

async fn fetch_page(user: &FwUser, page: u16, page_type: FwPageType, fw_client: &Client, pages: &mut Pages) {
        let mut fw_page = FwPage::new(page as u8, page_type, user, fw_client).await;
        fw_page.scrape_voteboxes(fw_client).await.unwrap();
        pages.films.push(fw_page);
}

async fn imdb_id_and_export(pages: Vec<FwPage>, fw_client: &Client, imdb_client: &Client, export_files: &mut ExportFiles) {    for page in pages {
        for mut title in page.rated_titles {
            title.get_title_fw_duration(fw_client).await;
            title.get_imdb_data_logic(imdb_client).await;
            print_title(&title);
            title.export_csv(export_files);
        }
    }
}

async fn scrape_fw(total_pages: u16, user: &FwUser, page_type: FwPageType, what: &str, fw_client: &Client, pages: &mut Pages) {
    for i in 1..=total_pages {
        fetch_page(user, i, page_type, fw_client, pages).await;
        if i != total_pages {
                print!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
            } else {
                println!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
            }
        io::stdout().flush().unwrap();
    }
}

fn print_title(title: &FwRatedTitle) {

    // Prints a rating with, or without a heart (if a title is favorited or not)
    let print_rating = || {
        match &title.rating.as_ref() {
            Some(api) => match api.favorite {
                true => format!("{}/10 ♥", api.rate).red(),
                false => format!("{}/10", api.rate).normal(),
            },
            None => "".to_string().normal(),
        }
    };

    match &title.imdb_data {
        Some(data) => println!(
            "{} {} {} {}{}",
            "[+]".green(),
            title.title_pl,
            print_rating(),
            "| ".dimmed(),
            data.id.as_ref().unwrap().dimmed()
        ),
        None => println!(
            "{} {} {}",
            "[-]".red(),
            title.title_pl,
            print_rating()
        ),
    }
}