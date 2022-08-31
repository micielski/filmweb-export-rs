use clap::Parser;
use colored::Colorize;
use reqwest::Client;
use std::{error::Error, io, io::Write, sync::Arc};
use tokio::sync::Mutex;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let mut exported_pages: Vec<FwPage> = Vec::new();
    let mut export_files = ExportFiles::default();
    let user = FwUser::new(args.username, args.token, args.session, args.jwt).await;
    let fw_client = filmweb_client_builder(&user).unwrap();
    let imdb_client = imdb_client_builder().unwrap();


    // Get count of rated films, and convert it to number of pages
    let counts = user.get_counts(&fw_client).await?;
    let films_pages = (counts.0 / 25 + 1) as u16;
    let serials_pages = (counts.1 / 25 + 1) as u16;
    let wants2see_pages = (counts.2 / 25 + 1) as u16;

    // BEGINNING OF SCRAPING ACTUAL DATA FROM Filmweb //
    print!("\r{} Scraping films...", "[i]".blue());
    scrape_fw(
        films_pages,
        &user,
        FwPageType::Films,
        "films",
        &fw_client,
        &mut exported_pages,
    )
    .await;

    print!("\r{} Scraping serials...", "[i]".blue());
    scrape_fw(
        serials_pages,
        &user,
        FwPageType::Serials,
        "serials",
        &fw_client,
        &mut exported_pages,
    )
    .await;

    print!("\r{} Scraping wants2see...", "[i]".blue());
    scrape_fw(
        wants2see_pages,
        &user,
        FwPageType::WantsToSee,
        "wants2see",
        &fw_client,
        &mut exported_pages,
    )
    .await;
    // END OF SCRAPING DATA FROM Filmweb //

    get_imdb_data_and_to_file(&mut exported_pages, &fw_client, &imdb_client, &mut export_files).await;

    println!("These following titles were unexported:");
    print_unexported(&exported_pages).await;

    Ok(())
}

// async fn get_imdb_id_and_export(
//     pages: &mut Vec<FwPage>,
//     fw_client: &Client,
//     imdb_client: &Client,
//     export_files: &mut ExportFiles,
// ) {
//     for page in &mut *pages {
//         for title in &mut *page.rated_titles {
//             title.get_title_fw_duration(fw_client).await;
//             title.get_imdb_data_logic(imdb_client).await;
//             print_title(title);
//             title.export_csv(export_files);
//         }
//     }
// }
async fn fetch_page(user: &FwUser, page: u16, page_type: FwPageType, fw_client: &Client, pages: &mut Vec<FwPage>) {
    let mut fw_page = FwPage::new(page as u8, page_type, user, fw_client).await;
    fw_page.scrape_voteboxes(fw_client).await.unwrap();
    pages.push(fw_page);
}

async fn get_imdb_data_and_to_file(
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
    total_pages: u16,
    user: &FwUser,
    page_type: FwPageType,
    what: &str,
    fw_client: &Client,
    pages: &mut Vec<FwPage>,
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

//  async fn scrape_fw(
//      total_pages: u8,
//      user: &FwUser,
//      page_type: FwPageType,
//      what: &str,
//      fw_client: &Client,
//      pages: &mut Vec<FwPage>,
//  ) {
//     let mut futures = Vec::new();
//     for i in 1..=total_pages {
//         let fw_client_copy = Arc::clone(&fw_client);
//         let fw_user_copy = Arc::clone(&user);
//         let fw_pages_copy = Arc::clone(&pages);
//         thread::sleep(time::Duration::from_millis(5000));
//         futures.push(tokio::spawn(async move {
//             fw_pages_copy
//                 .lock()
//                 .await
//                 .push(FwPage::new(i, page_type, &fw_user_copy, &*fw_client_copy).await);
//         }));
//     }
//   join_all(futures).await;
//      for i in 1..=total_pages {
//          pages.push(FwPage::new(i, page_type, &*user, &*fw_client).await);
//          pages[i as usize - 1].scrape_voteboxes(fw_client).await.unwrap();
//          if i == total_pages {
//              println!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
//          } else {
//              print!("\r{} Scraping {}... [{}/{}]", "[i]".blue(), what, i, total_pages);
//          }
//          io::stdout().flush().unwrap();
//      }
// }

async fn print_unexported(pages: &Vec<FwPage>) {
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
            "{} {} {} {}{} {}",
            "[+]".green(),
            title.title_pl,
            print_rating(),
            "-> | ".dimmed(),
            title.imdb_data.as_ref().unwrap().title.dimmed(),
            data.id.dimmed(),
        ),
        None => println!("{} {} {}", "[-]".red(), title.title_pl, print_rating()),
    }
}

#[tokio::test]
async fn my_test() {

}
