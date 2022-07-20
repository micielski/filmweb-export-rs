use csv::Writer;
use regex::Regex;
use reqwest::{header, Client};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, fs, fs::File};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux i686; rv:101.0) Gecko/20100101 Firefox/101.0";

#[derive(Debug)]
pub struct FwErrors;

impl Error for FwErrors {}

impl fmt::Display for FwErrors {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Title not found")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FwPageType {
    Films,
    Serials,
    WantsToSee,
}

pub struct FwUser {
    username: String,
    token: String,
    session: String,
    jwt: String,
}

pub struct FwPage {
    pub page_type: FwPageType,
    pub page: u8,
    page_source: Html,
    pub rated_titles: Vec<FwRatedTitle>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FwApiDetails {
    pub rate: u8,
    pub favorite: bool,
    #[serde(rename = "viewDate")]
    pub view_date: u32,
    pub timestamp: u128,
}

#[derive(Debug)]
pub struct IMDbApiDetails {
    pub id: Option<String>,
    pub duration: Option<u16>,
}

#[derive(Debug)]
pub struct FwRatedTitle {
    pub fw_url: String,
    pub title_id: u32,
    pub title_pl: String,
    pub title_orig: Option<String>,
    pub title_type: FwPageType,
    pub duration: Option<u16>, // time in minutes
    pub year: u16,
    pub rating: Option<FwApiDetails>,
    pub imdb_data: Option<IMDbApiDetails>,
}

pub struct ExportFiles {
    pub generic: Writer<File>,
    pub want2see: Writer<File>,
    pub favorited: Writer<File>,
}

impl FwUser {
    pub async fn new(username: String, token: String, session: String, jwt: String) -> Self {
        FwUser {
            username,
            token,
            session,
            jwt,
        }
    }

    pub async fn get_counts(&self, fw_client: &Client) -> Result<(u16, u16, u16), Box<dyn std::error::Error>> {
        let user_source = fw_client
            .get(format!("https://www.filmweb.pl/user/{}", self.username))
            .send()
            .await?
            .text()
            .await?;
        let user_source = Html::parse_document(user_source.as_str());
        let film_count: u16 = user_source
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-filmratedcount")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let serials_count: u16 = user_source
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-serialratedcount")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let want2see_count: u16 = user_source
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-filmw2scount")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        Ok((film_count, serials_count, want2see_count))
    }
}

impl FwPage {
    pub async fn new(page: u8, page_type: FwPageType, user: &FwUser, fw_client: &Client) -> Self {
        let page_source = FwPage::get_filmweb_page(user, page, &page_type, fw_client)
            .await
            .unwrap();
        FwPage {
            page,
            page_type,
            page_source,
            rated_titles: Vec::new(),
        }
    }

    async fn get_filmweb_page(
        user: &FwUser,
        fw_page: u8,
        fw_page_type: &FwPageType,
        fw_client: &Client,
    ) -> Result<Html, Box<dyn std::error::Error>> {
        let filmweb_user = match fw_page_type {
            FwPageType::Films => {
                fw_client
                    .get(format!(
                        "https://www.filmweb.pl/user/{}/films?page={}",
                        user.username, fw_page
                    ))
                    .send()
                    .await?
                    .text()
                    .await?
            }
            FwPageType::Serials => {
                fw_client
                    .get(format!(
                        "https://www.filmweb.pl/user/{}/serials?page={}",
                        user.username, fw_page
                    ))
                    .send()
                    .await?
                    .text()
                    .await?
            }
            FwPageType::WantsToSee => {
                fw_client
                    .get(format!(
                        "https://www.filmweb.pl/user/{}/wantToSee?page={}",
                        user.username, fw_page
                    ))
                    .send()
                    .await?
                    .text()
                    .await?
            }
        };

        return Ok(Html::parse_document(filmweb_user.as_str()));
    }

    pub async fn scrape_voteboxes(&mut self, fw_client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        for votebox in self.page_source.select(&Selector::parse("div.myVoteBox").unwrap()) {
            let title_id = votebox
                .select(&Selector::parse(".previewFilm").unwrap())
                .next()
                .unwrap()
                .value()
                .attr("data-film-id")
                .unwrap();
            let year = votebox
                .select(&Selector::parse(".preview__year").unwrap())
                .next()
                .unwrap()
                .inner_html();
            let title_pl = votebox
                .select(&Selector::parse(".preview__link").unwrap())
                .next()
                .unwrap()
                .inner_html();
            let title = votebox
                .select(&Selector::parse(".preview__originalTitle").unwrap())
                .next()
                .map(|element| element.inner_html());
            let url: String = format!(
                "https://filmweb.pl{}",
                votebox
                    .select(&Selector::parse(".preview__link").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap()
            );

            // async closures, when?
            let api_response = match self.page_type {
                FwPageType::Films => Some(
                    fw_client
                        .get(format!(
                            "https://www.filmweb.pl/api/v1/logged/vote/film/{}/details",
                            title_id
                        ))
                        .send(),
                ),
                FwPageType::Serials => Some(
                    fw_client
                        .get(format!(
                            "https://www.filmweb.pl/api/v1/logged/vote/serial/{}/details",
                            title_id
                        ))
                        .send(),
                ),
                FwPageType::WantsToSee => None,
            };

            // JWT could be invalidated meanwhile
            let rating: Option<FwApiDetails> = match api_response {
                Some(response) => match response.await?.json().await {
                    Ok(v) => v,
                    Err(e) => panic!("Provided JWT is invalid, {}", e),
                },
                None => None,
            };

            let title_id = title_id.parse::<u32>().unwrap();
            self.rated_titles.push(FwRatedTitle::new(
                url,
                title_id,
                title_pl,
                title,
                self.page_type,
                None,
                year.parse::<u16>().unwrap(),
                rating,
            ));
        }
        Ok(())
    }
}

impl FwRatedTitle {
    fn new(
        fw_url: String,
        title_id: u32,
        title_pl: String,
        title_orig: Option<String>,
        title_type: FwPageType,
        duration: Option<u16>,
        year: u16,
        rating: Option<FwApiDetails>,
    ) -> Self {
        FwRatedTitle {
            fw_url,
            title_id,
            title_pl,
            title_orig,
            title_type,
            duration,
            year,
            rating,
            imdb_data: None,
        }
    }

    pub async fn get_title_fw_duration(&mut self, fw_client: &Client) {
        let response = fw_client.get(&self.fw_url).send().await.unwrap().text().await.unwrap();
        let response = Html::parse_document(response.as_str());
        let duration = response
            .select(&Selector::parse(".filmCoverSection__duration").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-duration")
            .unwrap()
            .parse::<u16>();

        self.duration = duration.ok();
    }

    // because async closures don't exist yet
    pub async fn get_imdb_data_logic(&mut self, imdb_client: &Client) {
        self.imdb_data = match &self.title_orig {
            Some(title) => match self.get_imdb_data_advanced(title, imdb_client).await {
                Ok(api) => Some(api),
                Err(_) => match self.get_imdb_data_advanced(&self.title_pl, imdb_client).await {
                    Ok(api) => Some(api),
                    Err(_) => match self.get_imdb_data(&self.title_pl, imdb_client).await {
                        Ok(api) => Some(api),
                        Err(_) => None,
                    },
                },
            },
            None => match self.get_imdb_data_advanced(&self.title_pl, imdb_client).await {
                Ok(api) => Some(api),
                Err(_) => match self.get_imdb_data(&self.title_pl, imdb_client).await {
                    Ok(api) => Some(api),
                    Err(_) => None,
                },
            },
        };

        if let Some(api) = &mut self.imdb_data {
            // Compare duration of a title on both sites to check if it's ok 100%
            if let (Some(imdb_duration), Some(fw_duration)) = (api.duration, self.duration) {
                let upper;
                let lower;
                // if true, it's probably a tv show, and they seem to be very different on both sites
                // so let's be less restrictive then
                if imdb_duration <= 60 && fw_duration <= 60 {
                    upper = imdb_duration as f64 * 1.50;
                    lower = imdb_duration as f64 * 0.50;
                } else {
                    upper = imdb_duration as f64 * 1.15;
                    lower = imdb_duration as f64 * 0.85;
                }
                // if imdb duration doesn't fit into fw's then set it to none
                if upper < fw_duration.into() && lower > fw_duration.into() {
                    api.id = None;
                }
            }
        }
    }

    pub async fn get_imdb_data_advanced(
        &self,
        title: &String,
        imdb_client: &Client,
    ) -> Result<IMDbApiDetails, Box<FwErrors>> {
        let url = format!(
            "https://www.imdb.com/search/title/?title={}&release_date={},{}&adult=include",
            title, self.year, self.year
        );
        let tag = "div.lister-item-image";

        let response = imdb_client.get(url).send().await.unwrap().text().await.unwrap();
        let response = Html::parse_document(response.as_str());
        let title_id = match response.select(&Selector::parse(tag).unwrap()).next() {
            Some(id) => id,
            None => return Err(Box::new(FwErrors)),
        };
        let title_id = title_id.inner_html();
        let re = Regex::new(r"(\d{7,8})").unwrap();
        let title_id = format!(
            "{:08}",
            re.captures(title_id.as_str()).unwrap().get(0).unwrap().as_str()
        );

        let duration = {
            let x = match response.select(&Selector::parse(".runtime").unwrap()).next() {
                Some(a) => a.inner_html().replace(" min", ""),
                None => return Err(Box::new(FwErrors)),
            };
            match x.parse::<u16>() {
                Ok(x) => Some(x),
                Err(_) => return Err(Box::new(FwErrors)),
            }
        };

        let imdb_data = IMDbApiDetails {
            id: Some(title_id.trim().parse::<u32>().unwrap().to_string()),
            duration,
        };

        Ok(imdb_data)
    }

    pub async fn get_imdb_data(&self, title: &String, imdb_client: &Client) -> Result<IMDbApiDetails, Box<FwErrors>> {
        let url = format!("https://www.imdb.com/find?q={}", title);
        let tag = ".result_text";

        let response = imdb_client.get(url).send().await.unwrap().text().await.unwrap();
        let response = Html::parse_document(response.as_str());
        let title_id = match response.select(&Selector::parse(tag).unwrap()).next() {
            Some(id) => id,
            None => return Err(Box::new(FwErrors)),
        };
        let title_id = title_id.inner_html();
        let re = Regex::new(r"(\d{7,8})").unwrap();
        let title_id = format!(
            "{:08}",
            re.captures(title_id.as_str()).unwrap().get(0).unwrap().as_str()
        );

        let duration = None;

        let imdb_data = IMDbApiDetails {
            id: Some(title_id.trim().parse::<u32>().unwrap().to_string()),
            duration,
        };

        Ok(imdb_data)
    }

    pub fn export_csv(self, files: &mut ExportFiles) {
        let title = self.title_orig.unwrap_or(self.title_pl);

        let rating = self
            .rating
            .as_ref()
            .map(|r| r.rate.to_string())
            .unwrap_or_else(|| "no-vote".to_string());

        let imdb_id = self.imdb_data.unwrap().id.unwrap_or_else(|| "not-found".to_string());

        let year = self.year.to_string();

        let mut fields = ["", "", "", "", "", "", "", "", "", "", "", "", ""];
        fields[0] = imdb_id.as_ref();
        fields[1] = rating.as_ref();
        fields[3] = title.as_ref();
        fields[9] = year.as_ref();
        let write_title = |file: &mut Writer<File>| {
            file.write_record(fields).unwrap();
            file.flush().unwrap();
        };

        match self.rating {
            Some(yes) => match yes.favorite {
                true => write_title(&mut files.favorited),
                false => write_title(&mut files.generic),
            },
            None => write_title(&mut files.want2see),
        }
    }
}

pub fn filmweb_client_builder(user: &FwUser) -> Result<Client, reqwest::Error> {
    let cookies = format!(
        "_fwuser_token={}; _fwuser_sessionId={}; JWT={}",
        user.token, user.session, user.jwt
    );

    let mut headers = header::HeaderMap::new();
    headers.insert(header::COOKIE, header::HeaderValue::from_str(&cookies).unwrap());
    headers.insert(header::CONNECTION, header::HeaderValue::from_static("keep-alive"));
    headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .gzip(true)
        .default_headers(headers)
        .cookie_store(true)
        .build()
}

pub fn imdb_client_builder() -> Result<Client, reqwest::Error> {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONNECTION, header::HeaderValue::from_static("keep-alive"));
    headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .gzip(true)
        .default_headers(headers)
        .cookie_store(true)
        .build()
}

impl ExportFiles {
    pub fn new() -> Self {
        let write_header = |wtr| -> Writer<File> {
            let mut wtr: Writer<File> = csv::Writer::from_writer(wtr);
            wtr.write_record(&[
                "Const",
                "Your Rating",
                "Date Rated",
                "Title",
                "URL",
                "Title Type",
                "IMDb Rating",
                "Runtime (mins)",
                "Year",
                "Genres",
                "Num Votes",
                "Release Date",
                "Directors",
            ])
            .unwrap();
            wtr
        };
        let _ = fs::create_dir("./exports");
        let generic = File::create("exports/generic.csv").unwrap();
        let want2see = File::create("exports/want2see.csv").unwrap();
        let favorited = File::create("exports/favorited.csv").unwrap();
        let generic = write_header(generic);
        let want2see = write_header(want2see);
        let favorited = write_header(favorited);
        ExportFiles {
            generic,
            want2see,
            favorited,
        }
    }
}

impl Default for ExportFiles {
    fn default() -> Self {
        Self::new()
    }
}
