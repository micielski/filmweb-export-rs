use csv::Writer;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{fs, fs::File};
use thiserror::Error;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux i686; rv:101.0) Gecko/20100101 Firefox/101.0";

#[derive(Error, Debug)]
pub enum FwErrors {
    #[error("title not found")]
    ZeroResults,
    #[error("couldn't fetch duration")]
    InvalidDuration,
    #[error("provided JWT is invalid/invalidated")]
    InvalidJwt,
    #[error("year parsing error")]
    InvalidYear { title_id: u32, failed_year: String },
}

#[derive(Clone, Copy, Debug)]
pub enum FwTitleType {
    Film,
    Serial,
    WantsToSee,
}

#[derive(Clone, Copy, Debug)]
pub enum FwPageNumber {
    Films(u8),
    Serials(u8),
    WantsToSee(u8),
}

impl From<FwPageNumber> for FwTitleType {
    fn from(fw_page_number: FwPageNumber) -> Self {
        match fw_page_number {
            FwPageNumber::Films(_) => Self::Film,
            FwPageNumber::Serials(_) => Self::Serial,
            FwPageNumber::WantsToSee(_) => Self::WantsToSee,
        }
    }
}

#[derive(Debug)]
pub struct FwUser {
    pub username: String,
    pub token: String,
    pub session: String,
    pub jwt: String,
    pub titles_count: Option<TitlesCount>,
}

#[derive(Debug, Clone, Copy)]
pub struct TitlesCount {
    pub films: u16,
    pub serials: u16,
    pub marked_to_see: u16,
}

#[derive(Debug)]
pub struct FwPage {
    pub page_type: FwPageNumber,
    page_source: String,
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
    pub title: String,
    pub id: String,
    pub duration: u32,
}

#[derive(Debug)]
pub struct FwRatedTitle {
    pub fw_url: String,
    pub fw_title_id: u32,
    pub fw_title_pl: String,
    pub fw_title_orig: Option<String>,
    pub title_type: FwTitleType,
    pub fw_duration: Option<u16>, // time in minutes
    pub year: Year,
    pub rating: Option<FwApiDetails>,
    pub imdb_data: Option<IMDbApiDetails>,
}

#[derive(Debug)]
pub enum Year {
    OneYear(u16),
    Range(u16, u16),
}

#[derive(Debug)]
pub struct ExportFiles {
    pub generic: Writer<File>,
    pub want2see: Writer<File>,
    pub favorited: Writer<File>,
}

impl FwUser {
    #[must_use]
    pub fn new(username: String, token: String, session: String, jwt: String) -> Self {
        Self {
            username,
            token,
            session,
            jwt,
            titles_count: None,
        }
    }
    pub fn filmweb_client_builder(&self) -> Result<Client, reqwest::Error> {
        log::debug!("Creating Filmweb Client");
        let cookies = format!(
            "_fwuser_token={}; _fwuser_sessionId={}; JWT={};",
            self.token, self.session, self.jwt
        );

        let mut headers = header::HeaderMap::new();
        headers.insert(header::COOKIE, header::HeaderValue::from_str(&cookies).unwrap());
        headers.insert(header::CONNECTION, header::HeaderValue::from_static("keep-alive"));
        headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

        Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .default_headers(headers)
            .cookie_store(true)
            .build()
    }

    pub fn get_counts(&mut self, fw_client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        let response = {
            let unparsed_response = fw_client
                .get(format!("https://www.filmweb.pl/user/{}", self.username))
                .send()?
                .text()?;
            Html::parse_document(unparsed_response.as_str())
        };
        let films: u16 = response
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-filmratedcount")
            .unwrap()
            .parse::<u16>()?;
        let serials: u16 = response
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-serialratedcount")
            .unwrap()
            .parse::<u16>()?;
        let marked_to_see: u16 = response
            .select(&Selector::parse(".VoteStatsBox").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("data-filmw2scount")
            .unwrap()
            .parse::<u16>()?;
        self.titles_count = Some(TitlesCount {
            films,
            serials,
            marked_to_see,
        });
        Ok(())
    }
}

impl TitlesCount {}

impl FwPage {
    #[must_use]
    pub fn new(page_type: FwPageNumber, user: &FwUser, fw_client: &Client) -> Self {
        let page_source = Self::get_filmweb_page(user, page_type, fw_client).unwrap();
        Self {
            page_type,
            page_source,
            rated_titles: Vec::new(),
        }
    }

    fn get_filmweb_page(
        user: &FwUser,
        page: FwPageNumber,
        fw_client: &Client,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let filmweb_user = match page {
            FwPageNumber::Films(page) if page != 0 => fw_client
                .get(format!(
                    "https://www.filmweb.pl/user/{}/films?page={}",
                    user.username, page
                ))
                .send()
                .unwrap()
                .text()
                .unwrap(),
            FwPageNumber::Serials(page) if page != 0 => fw_client
                .get(format!(
                    "https://www.filmweb.pl/user/{}/serials?page={}",
                    user.username, page
                ))
                .send()
                .unwrap()
                .text()
                .unwrap(),
            FwPageNumber::WantsToSee(page) if page != 0 => fw_client
                .get(format!(
                    "https://www.filmweb.pl/user/{}/wantToSee?page={}",
                    user.username, page
                ))
                .send()?
                .text()?,
            _ => panic!("Page cannot be 0"),
        };
        Ok(filmweb_user)
    }

    pub fn scrape_from_page(&mut self, fw_client: &Client) -> Result<(), FwErrors> {
        assert!(self.page_source.contains("preview__alternateTitle"));
        assert!(self.page_source.contains("preview__year"));
        assert!(self.page_source.contains("preview__link"));
        let html = Html::parse_document(&self.page_source);
        for votebox in html.select(&Selector::parse("div.myVoteBox").unwrap()) {
            let fw_title_id = {
                let fw_title_id = votebox
                    .select(&Selector::parse(".previewFilm").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("data-film-id")
                    .unwrap();
                fw_title_id.trim().parse::<u32>().unwrap()
            };

            let year = {
                let year = votebox
                    .select(&Selector::parse(".preview__year").unwrap())
                    .next()
                    .unwrap()
                    .inner_html();
                // Parse year properly, set it to Year::Range if year is in a format for example, 2015-2019
                // It's used in serials only
                if year.contains('-') {
                    let years = year.trim().split('-').collect::<Vec<&str>>();
                    let year_start = years[0]
                        .trim()
                        .parse::<u16>()
                        .expect("Failed to parse a year from a serial votebox");
                    let year_end = match years[1].trim().parse::<u16>() {
                        Ok(year) => year,
                        Err(_) => year_start,
                    };
                    Year::Range(year_start, year_end)
                } else {
                    match year.trim().parse::<u16>() {
                        Ok(year) => Year::OneYear(year),
                        Err(_) => {
                            return Err(FwErrors::InvalidYear {
                                title_id: fw_title_id,
                                failed_year: year,
                            })
                        }
                    }
                }
            };

            let fw_title_pl = votebox
                .select(&Selector::parse(".preview__link").unwrap())
                .next()
                .unwrap()
                .inner_html();

            let fw_title_orig = votebox
                .select(&Selector::parse(".preview__alternateTitle").unwrap())
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

            let rating: Option<FwApiDetails> = {
                let api_response = match self.page_type {
                    FwPageNumber::Films(_) => Some(
                        fw_client
                            .get(format!(
                                "https://www.filmweb.pl/api/v1/logged/vote/film/{}/details",
                                fw_title_id
                            ))
                            .send(),
                    ),
                    FwPageNumber::Serials(_) => Some(
                        fw_client
                            .get(format!(
                                "https://www.filmweb.pl/api/v1/logged/vote/serial/{}/details",
                                fw_title_id
                            ))
                            .send(),
                    ),
                    FwPageNumber::WantsToSee(_) => None,
                };

                // JWT could be invalidated meanwhile
                match api_response {
                    Some(response) => match response.unwrap().json() {
                        Ok(v) => v,
                        Err(_) => return Err(FwErrors::InvalidJwt),
                    },
                    None => None,
                }
            };

            let fw_duration = {
                let document = {
                    let response = fw_client.get(&url).send().unwrap().text().unwrap();
                    Html::parse_document(response.as_str())
                };
                match document
                    .select(&Selector::parse(".filmCoverSection__duration").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("data-duration")
                    .unwrap()
                    .parse::<u16>()
                {
                    Ok(mins) => Some(mins),
                    Err(_) => None,
                }
            };

            self.rated_titles.push(FwRatedTitle {
                fw_url: url.clone(),
                fw_title_id,
                fw_title_pl,
                fw_title_orig,
                title_type: self.page_type.into(),
                fw_duration,
                year,
                rating,
                imdb_data: None,
            });
        }
        Ok(())
    }
}

impl FwRatedTitle {
    #[must_use]
    pub fn is_duration_ok(&self) -> bool {
        let imdb_duration = match &self.imdb_data {
            None => return false,
            Some(imdb_api) => f64::from(imdb_api.duration),
        };

        let fw_duration = match self.fw_duration {
            None => return true,
            Some(duration) => duration,
        };

        let upper;
        let lower;
        // if true, it's probably a tv show, and they seem to be very different on both sites
        // so let's be less restrictive then
        if imdb_duration <= 60_f64 && fw_duration <= 60_u16 {
            upper = imdb_duration * 1.50;
            lower = imdb_duration * 0.50;
        } else {
            upper = imdb_duration * 1.15;
            lower = imdb_duration * 0.85;
        }
        // if imdb duration doesn't fit into fw's then set it to none
        if upper >= fw_duration.into() && lower >= fw_duration.into() {
            return false;
        }
        true
    }

    pub fn get_imdb_data_logic(&mut self, imdb_client: &Client) {
        let year = match self.year {
            Year::OneYear(year) | Year::Range(year, _) => year,
        };
        self.imdb_data = match &self.fw_title_orig {
            Some(title) => match self.get_imdb_data_advanced(title, year, year, imdb_client) {
                Ok(api) => Some(api),
                Err(_) => match self.get_imdb_data_advanced(&self.fw_title_pl, year, year, imdb_client) {
                    Ok(api) => Some(api),
                    Err(_) => match self.get_imdb_data(&self.fw_title_pl, year, imdb_client) {
                        Ok(api) => Some(api),
                        Err(_) => match self.get_imdb_data(title, year, imdb_client) {
                            Ok(api) => Some(api),
                            Err(_) => None,
                        },
                    },
                },
            },
            None => {
                log::info!("{} doesn't contain original title", self.fw_title_pl);
                match self.get_imdb_data_advanced(&self.fw_title_pl, year, year, imdb_client) {
                    Ok(api) => Some(api),
                    Err(_) => match self.get_imdb_data(&self.fw_title_pl, year, imdb_client) {
                        Ok(api) => Some(api),
                        Err(_) => None,
                    },
                }
            }
        }
    }

    pub fn get_imdb_data_advanced(
        &self,
        title: &str,
        year_start: u16,
        year_end: u16,
        imdb_client: &Client,
    ) -> Result<IMDbApiDetails, Box<dyn std::error::Error>> {
        let url = format!(
            "https://www.imdb.com/search/title/?title={}&release_date={},{}&adult=include",
            title, year_start, year_end
        );

        let document = {
            let response = imdb_client.get(&url).send()?.text()?;
            Html::parse_document(response.as_str())
        };

        let title_data = match document
            .select(&Selector::parse("div.lister-item-image").unwrap())
            .next()
        {
            Some(id) => id,
            None => {
                log::error!("Failed to get a match in Fn get_imdb_data_advanced for {title} {year_start} on {url}");
                return Err(Box::new(FwErrors::ZeroResults));
            }
        };

        let title_id = {
            let id = title_data.inner_html();
            let regex = Regex::new(r"(\d{7,8})").unwrap();
            format!("{:08}", regex.captures(id.as_str()).unwrap().get(0).unwrap().as_str())
        };
        log::debug!("Found a potential IMDb id for {title} {year_start} on {url}");

        let imdb_title = document
            .select(&Selector::parse("img.loadlate").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("alt")
            .unwrap();

        let duration = {
            let x = match document.select(&Selector::parse(".runtime").unwrap()).next() {
                Some(a) => a.inner_html().replace(" min", ""),
                None => {
                    log::error!("Failed to fetch duration for {title} {year_start} on {url}");
                    return Err(Box::new(FwErrors::InvalidDuration));
                }
            };
            match x.parse::<u32>() {
                Ok(x) => x,
                Err(_) => {
                    log::error!("Failed parsing duration to int for {title} {year_start} on {url}");
                    return Err(Box::new(FwErrors::InvalidDuration));
                }
            }
        };
        let imdb_data = IMDbApiDetails {
            id: title_id.trim().parse::<u32>()?.to_string(),
            title: imdb_title.to_string(),
            duration,
        };

        Ok(imdb_data)
    }

    pub fn get_imdb_data(
        &self,
        title: &str,
        year: u16,
        imdb_client: &Client,
    ) -> Result<IMDbApiDetails, Box<dyn std::error::Error>> {
        let url_query = format!("https://www.imdb.com/find?q={}+{}", title, year);
        let document = {
            let response = imdb_client.get(&url_query).send()?.text()?;
            Html::parse_document(response.as_str())
        };
        let imdb_title = match document.select(&Selector::parse(".result_text a").unwrap()).next() {
            Some(title) => title.inner_html(),
            None => {
                log::error!("No results in Fn get_imdb_data for {title} {year} on {url_query}");
                return Err(Box::new(FwErrors::ZeroResults));
            }
        };

        let title_id = match document.select(&Selector::parse(".result_text").unwrap()).next() {
            Some(id) => id,
            None => {
                log::error!("No results in Fn get_imdb_data for {title} {year} on {url_query}");
                return Err(Box::new(FwErrors::ZeroResults));
            }
        };

        // get url of a title, and grab the duration
        let url = {
            let url_suffix = document
                .select(&Selector::parse("td.result_text a").unwrap())
                .next()
                .unwrap()
                .value()
                .attr("href")
                .unwrap();
            format!("https://www.imdb.com{}", url_suffix)
        };

        let document = {
            let response = imdb_client.get(&url).send()?.text()?;
            Html::parse_document(response.as_str())
        };

        let get_dirty_duration = |nth| {
            document
                .select(&Selector::parse(".ipc-inline-list__item").unwrap())
                .nth(nth)
                .expect("Panic occured while trying to export {title} {year}")
                .inner_html()
        };

        let mut dirty_duration = get_dirty_duration(5);
        if dirty_duration.contains("Unrated") || dirty_duration.contains("Not Rated") || dirty_duration.contains("TV") {
            dirty_duration = get_dirty_duration(6);
        }

        if dirty_duration.len() > 40 {
            log::error!("Invalid duration in Fn get_imdb_data on {url} for {title} {year} source: {url_query}");
            return Err(Box::new(FwErrors::InvalidDuration));
        }

        // Example of dirty_duration: 1<!-- -->h<!-- --> <!-- -->33<!-- -->m<
        let duration = {
            let dirty_duration: Vec<u32> = dirty_duration
                .replace("<!-- -->", " ")
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect();
            if dirty_duration.len() >= 2 {
                dirty_duration[0] * 60 + dirty_duration[1]
            } else {
                dirty_duration[0]
            }
        };
        log::debug!("Found duration {duration}m for {title} {year}");

        let title_id = title_id.inner_html();
        let re = Regex::new(r"(\d{7,8})").unwrap();
        let title_id = format!(
            "{:08}",
            re.captures(title_id.as_str()).unwrap().get(0).unwrap().as_str()
        );

        let imdb_data = IMDbApiDetails {
            id: title_id.trim().parse::<u32>().unwrap().to_string(),
            title: imdb_title,
            duration,
        };

        Ok(imdb_data)
    }

    pub fn export_csv(&self, files: &mut ExportFiles) {
        let title = self.fw_title_orig.as_ref().unwrap_or(&self.fw_title_pl);

        let rating = self
            .rating
            .as_ref()
            .map_or_else(|| "no.vote".to_string(), |r| r.rate.to_string());

        let imdb_id = {
            if self.imdb_data.is_some() {
                &self.imdb_data.as_ref().unwrap().id
            } else {
                "not-found"
            }
        };

        // In case of year being a range, set it to the first one
        let year = match self.year {
            Year::OneYear(year) | Year::Range(year, _) => year.to_string(),
        };

        log::debug!(
            "Exporting to CSV title: {}, rating: {}, imdb_id: {}",
            title,
            rating,
            imdb_id
        );
        let mut fields = [""; 13];
        fields[0] = imdb_id;
        fields[1] = rating.as_ref();
        fields[3] = title.as_ref();
        fields[9] = year.as_ref();
        let write_title = |file: &mut Writer<File>| {
            file.write_record(fields).unwrap();
            file.flush().unwrap();
        };

        match &self.rating {
            Some(yes) => {
                if yes.favorite {
                    write_title(&mut files.favorited);
                } else {
                    write_title(&mut files.generic);
                }
            }
            None => write_title(&mut files.want2see),
        }
    }
}

pub fn imdb_client_builder() -> Result<Client, reqwest::Error> {
    log::debug!("Creating IMDb Client");
    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONNECTION, header::HeaderValue::from_static("keep-alive"));
    headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

    Client::builder()
        .user_agent(USER_AGENT)
        .gzip(true)
        .default_headers(headers)
        .cookie_store(true)
        .build()
}

impl ExportFiles {
    #[must_use]
    pub fn new() -> Result<Self, std::io::Error> {
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
        if let Err(e) = fs::create_dir("./exports") {
            match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => panic!("{}", e),
            }
        };
        let generic = File::create("exports/generic.csv")?;
        let want2see = File::create("exports/want2see.csv")?;
        let favorited = File::create("exports/favorited.csv")?;
        let generic = write_header(generic);
        let want2see = write_header(want2see);
        let favorited = write_header(favorited);
        Ok(Self {
            generic,
            want2see,
            favorited,
        })
    }
}

impl Default for ExportFiles {
    fn default() -> Self {
        Self::new().unwrap()
    }
}
