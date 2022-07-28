use clap::Parser;
use colored::Colorize;
use reqwest::Client;
use std::{error::Error, io, io::Write};

use filmweb_export_rs::{
    filmweb_client_builder, imdb_client_builder, ExportFiles, FwPage, FwPageType, FwRatedTitle, FwUser,
};

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

    let films_pages = counts.0 / 25 + 1;
    let serials_pages = counts.1 / 25 + 1;
    let wants2see_pages = counts.2 / 25 + 1;

    print!("\r{} Scraping films...", "[i]".blue());
    scrape_fw(films_pages, &user, FwPageType::Films, "films", &fw_client, &mut pages).await;

    print!("\r{} Scraping serials...", "[i]".blue());
    scrape_fw(
        serials_pages,
        &user,
        FwPageType::Serials,
        "serials",
        &fw_client,
        &mut pages,
    )
    .await;

    print!("\r{} Scraping wants2see...", "[i]".blue());
    scrape_fw(
        wants2see_pages,
        &user,
        FwPageType::WantsToSee,
        "wants2see",
        &fw_client,
        &mut pages,
    )
    .await;

    let mut all = Vec::new();
    all.extend(pages.films);
    all.extend(pages.serials);
    all.extend(pages.wants2see);
    // Obvious ways to minimize the code doesn't work
    imdb_id_and_export(&mut all, &fw_client, &imdb_client, &mut export_files).await;

    println!("These following titles were unexported:");
    print_unexported(&all);

    Ok(())
}

async fn fetch_page(user: &FwUser, page: u8, page_type: FwPageType, fw_client: &Client, pages: &mut Pages) {
    let mut fw_page = FwPage::new(page as u8, page_type, user, fw_client).await;
    fw_page.scrape_voteboxes(fw_client).await.unwrap();
    pages.films.push(fw_page);
}

async fn imdb_id_and_export(
    pages: &mut Vec<FwPage>,
    fw_client: &Client,
    imdb_client: &Client,
    export_files: &mut ExportFiles,
) {
    for page in pages {
        for title in &mut page.rated_titles {
            title.get_title_fw_duration(fw_client).await;
            title.get_imdb_data_logic(imdb_client).await;
            print_title(title);
            title.export_csv(export_files);
        }
    }
}

async fn scrape_fw(
    total_pages: u8,
    user: &FwUser,
    page_type: FwPageType,
    what: &str,
    fw_client: &Client,
    pages: &mut Pages,
) {
    for i in 1..=total_pages {
        fetch_page(user, i, page_type, fw_client, pages).await;
        if i == total_pages {
            println!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
        } else {
            print!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
        }
        io::stdout().flush().unwrap();
    }
}

fn print_unexported(pages: &Vec<FwPage>) {
    for page in pages {
        for title in &page.rated_titles {
            if title.imdb_data.as_ref().unwrap().id == "not-found" {
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

    match &title.imdb_data {
        Some(data) => println!(
            "{} {} {} {}{}",
            "[+]".green(),
            title.title_pl,
            print_rating(),
            "| ".dimmed(),
            data.id.dimmed()
        ),
        None => println!("{} {} {}", "[-]".red(), title.title_pl, print_rating()),
    }
}
