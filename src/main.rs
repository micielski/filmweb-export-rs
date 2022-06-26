use filmweb_export_rs::*;
use reqwest::Client;
use clap::Parser;
use std::error::Error;

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
    let mut export_files = ExportFiles::new();
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
    // let x = (1..=counts.0/25+1).into_iter().map(|i| fetch_page(&user, i, FwPageType::Films, &fw_client, &mut pages).await);

    for i in 1..=(counts.0 as f64/25 as f64 +1_f64.ceil()) as u16 {
      fetch_page(&user, i, FwPageType::Films, &fw_client, &mut pages).await;
      println!("{}", i);
    }

    for i in 1..=(counts.1 as f64/25 as f64 +1_f64.ceil()) as u16 {
        fetch_page(&user, i, FwPageType::Serials, &fw_client, &mut pages).await;
    }

    for i in 1..=(counts.2 as f64/25 as f64 +1_f64.ceil()) as u16 {
        fetch_page(&user, i, FwPageType::WantsToSee, &fw_client, &mut pages).await;
    }

    imdb_id_and_export(pages.films, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.serials, &imdb_client, &mut export_files).await;
    imdb_id_and_export(pages.wants2see, &imdb_client, &mut export_files).await;

    Ok(())
}

async fn fetch_page(user: &FwUser, page: u16, page_type: FwPageType, fw_client: &Client, pages: &mut Pages) {
    let mut stronka = FwPage::new(page as u8, page_type, &user, &fw_client).await;
    stronka.scrape_voteboxes(&fw_client).await.unwrap();
    pages.films.push(stronka);
}

async fn imdb_id_and_export(pages: Vec<FwPage>, imdb_client: &Client, export_files: &mut ExportFiles) {
    for page in pages {
        for mut tytul in page.rated_titles {
            tytul.get_imdb_ids_logic(&imdb_client).await;
            tytul.export_csv(export_files);
        }
    }
}