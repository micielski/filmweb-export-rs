use csv::Writer;
use priority_queue::PriorityQueue;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{fs, fs::File};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:106.0) Gecko/20100101 Firefox/106.0";

pub mod error;
pub use error::FwErrors;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FwTitleType {
    Film,
    Serial,
    WantsToSee,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FwPageNumbered {
    Films(u8),
    Serials(u8),
    WantsToSee(u8),
}

impl From<FwPageNumbered> for FwTitleType {
    fn from(fw_page_number: FwPageNumbered) -> Self {
        match fw_page_number {
            FwPageNumbered::Films(_) => Self::Film,
            FwPageNumbered::Serials(_) => Self::Serial,
            FwPageNumbered::WantsToSee(_) => Self::WantsToSee,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FwUser {
    pub username: String,
    pub token: String,
    pub session: String,
    pub jwt: String,
    // TODO: remove option
    pub counts: Option<UserCounts>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserCounts {
    pub movies: u16,
    pub shows: u16,
    pub marked_to_see: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FwPage {
    pub page: FwPageNumbered,
    pub rated_titles: Vec<FwRatedTitle>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FwApiDetails {
    pub rate: u8,
    pub favorite: bool,
    #[serde(rename = "viewDate")]
    pub view_date: u32,
    pub timestamp: u128,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct IMDbApiDetails {
    pub title: String,
    pub id: String,
    pub duration: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FwRatedTitle {
    pub fw_url: String,
    pub fw_id: u32,
    pub fw_title_pl: String,
    pub fw_alter_titles: Option<PriorityQueue<AlternateTitle, u8>>,
    pub title_type: FwTitleType,
    pub fw_duration: Option<u16>, // time in minutes
    pub year: Year,
    pub rating: Option<FwApiDetails>,
    pub imdb_data: Option<IMDbApiDetails>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlternateTitle {
    pub language: String,
    pub title: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
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
    pub const fn new(username: String, token: String, session: String, jwt: String) -> Self {
        Self {
            username,
            token,
            session,
            jwt,
            counts: None,
        }
    }

    pub fn get_username(fw_client: &Client) -> Result<String, FwErrors> {
        let res = fw_client.get("https://www.filmweb.pl/settings").send()?.text()?;
        let document = Html::parse_document(&res);
        let username = match document
            .select(&Selector::parse(".mainSettings__groupItemStateContent").unwrap())
            .nth(2)
        {
            Some(username_tag) => username_tag.inner_html().trim().to_owned(),
            None => return Err(FwErrors::InvalidCredentials),
        };
        Ok(username)
    }

    pub fn filmweb_client_builder(token: &str, session: &str, jwt: &str) -> Result<Client, FwErrors> {
        log::debug!("Creating Filmweb Client");
        let cookies = format!(
            "_fwuser_token={}; _fwuser_sessionId={}; JWT={};",
            token.trim(),
            session.trim(),
            jwt.trim()
        );

        let mut headers = header::HeaderMap::new();
        headers.insert(header::COOKIE, header::HeaderValue::from_str(&cookies)?);
        headers.insert(header::CONNECTION, header::HeaderValue::from_static("keep-alive"));
        headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

        Ok(Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .default_headers(headers)
            .cookie_store(true)
            .build()?)
    }

    pub fn get_counts(&mut self, fw_client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        let movies: u16 = fw_client
            .get(format!(
                "https://www.filmweb.pl/api/v1/user/{}/votes/film/count",
                self.username
            ))
            .send()?
            .text()?
            .parse()
            .unwrap();

        let marked_to_see_movies: u16 = fw_client
            .get(format!(
                "https://www.filmweb.pl/api/v1/user/{}/want2see/film/count",
                self.username
            ))
            .send()?
            .text()?
            .parse()
            .unwrap();

        let shows: u16 = fw_client
            .get(format!(
                "https://www.filmweb.pl/api/v1/user/{}/votes/serial/count",
                self.username
            ))
            .send()?
            .text()?
            .parse()
            .unwrap();

        let marked_to_see_shows: u16 = fw_client
            .get(format!(
                "https://www.filmweb.pl/api/v1/user/{}/want2see/serial/count",
                self.username
            ))
            .send()?
            .text()?
            .parse()
            .unwrap();
        let marked_to_see = marked_to_see_shows + marked_to_see_movies;
        self.counts = Some(UserCounts {
            movies,
            shows,
            marked_to_see,
        });
        // self.counts = Some(counts);
        Ok(())
    }
}

impl FwPage {
    pub const fn new(page_type: FwPageNumbered) -> Result<Self, FwErrors> {
        Ok(Self {
            page: page_type,
            rated_titles: Vec::new(),
        })
    }

    fn get_url(username: &str, page: FwPageNumbered) -> String {
        match page {
            FwPageNumbered::Films(page) if page != 0 => {
                format!("https://www.filmweb.pl/user/{}/films?page={}", username, page)
            }
            FwPageNumbered::Serials(page) if page != 0 => {
                format!("https://www.filmweb.pl/user/{}/serials?page={}", username, page)
            }
            FwPageNumbered::WantsToSee(page) if page != 0 => {
                format!("https://www.filmweb.pl/user/{}/wantToSee?page={}", username, page)
            }
            _ => panic!("Page mustn't be 0"),
        }
    }

    pub fn scrape(&mut self, username: &str, fw_client: &Client) -> Result<(), FwErrors> {
        let res = fw_client.get(Self::get_url(username, self.page)).send()?.text()?;
        assert!(res.contains("preview__alternateTitle"));
        assert!(res.contains("preview__year"));
        assert!(res.contains("preview__link"));
        let document = Html::parse_document(&res);
        for votebox in document.select(&Selector::parse("div.myVoteBox").unwrap()) {
            let fw_title_id = {
                let fw_title_id = votebox
                    .select(&Selector::parse(".previewFilm").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("data-film-id")
                    .unwrap();
                fw_title_id.trim().parse::<u32>()?
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
                    let year_end = years[1].trim().parse::<u16>().map_or(year_start, |year| year);
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

            let title_url: String = format!(
                "https://filmweb.pl{}",
                votebox
                    .select(&Selector::parse(".preview__link").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap()
            );

            let alternate_titles_url = format!("{}/titles", title_url);

            let rating: Option<FwApiDetails> = {
                let api_response = match self.page {
                    FwPageNumbered::Films(_) => Some(
                        fw_client
                            .get(format!(
                                "https://www.filmweb.pl/api/v1/logged/vote/film/{}/details",
                                fw_title_id
                            ))
                            .send(),
                    ),
                    FwPageNumbered::Serials(_) => Some(
                        fw_client
                            .get(format!(
                                "https://www.filmweb.pl/api/v1/logged/vote/serial/{}/details",
                                fw_title_id
                            ))
                            .send(),
                    ),
                    FwPageNumbered::WantsToSee(_) => None,
                };

                // JWT could be invalidated meanwhile
                match api_response {
                    Some(response) => match response?.json() {
                        Ok(v) => v,
                        Err(e) => {
                            log::info!("Bad Filmweb's api response: {e}");
                            return Err(FwErrors::InvalidJwt);
                        }
                    },
                    None => None,
                }
            };

            let fw_duration = {
                let document = {
                    let res = fw_client.get(&title_url).send()?.text()?;
                    Html::parse_document(&res)
                };
                document
                    .select(&Selector::parse(".filmCoverSection__duration").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("data-duration")
                    .unwrap()
                    .parse::<u16>()
                    .ok()
            };
            self.rated_titles.push(FwRatedTitle {
                fw_url: title_url.clone(),
                fw_id: fw_title_id,
                fw_title_pl,
                fw_alter_titles: Some(AlternateTitle::fw_get_titles(&alternate_titles_url, fw_client)?),
                title_type: self.page.into(),
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

        // if true, it's probably a tv show, and they seem to be very different on both sites
        // so let's be less restrictive then
        let (upper, lower) = if imdb_duration <= 60_f64 && fw_duration <= 60_u16 {
            (imdb_duration * 1.50, imdb_duration * 0.75)
        } else {
            (imdb_duration * 1.15, imdb_duration * 0.85)
        };

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

        'main: while let Some((ref alternate_title, score)) = self.fw_alter_titles.as_mut().unwrap().pop() {
            if score == u8::MIN {
                break;
            }
            for i in 1..=2 {
                if i % 2 == 1 {
                    if let Ok(imdb_data) = self.get_imdb_data_advanced(&alternate_title.title, year, year, imdb_client)
                    {
                        self.imdb_data = Some(imdb_data);
                        break 'main;
                    }
                } else if let Ok(imdb_data) = self.get_imdb_data(&alternate_title.title, year, imdb_client) {
                    self.imdb_data = Some(imdb_data);
                    break 'main;
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
            Html::parse_document(&response)
        };

        let title_data = if let Some(id) = document
            .select(&Selector::parse("div.lister-item-image").unwrap())
            .next()
        {
            id
        } else {
            log::info!("Failed to get a match in Fn get_imdb_data_advanced for {title} {year_start} on {url}");
            return Err(Box::new(FwErrors::ZeroResults));
        };

        let title_id = {
            let id = title_data.inner_html();
            let regex = Regex::new(r"(\d{7,8})").unwrap();
            format!("tt{:0>7}", &regex.captures(&id).unwrap()[0]).trim().to_string()
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
            let x = if let Some(a) = document.select(&Selector::parse(".runtime").unwrap()).next() {
                a.inner_html().replace(" min", "")
            } else {
                log::info!("Failed to fetch duration for {title} {year_start} on {url}");
                return Err(Box::new(FwErrors::InvalidDuration));
            };

            if let Ok(x) = x.parse::<u32>() {
                x
            } else {
                log::info!("Failed parsing duration to int for {title} {year_start} on {url}");
                return Err(Box::new(FwErrors::InvalidDuration));
            }
        };

        let imdb_data = IMDbApiDetails {
            id: title_id,
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
            Html::parse_document(&response)
        };

        let imdb_title = if let Some(title) = document.select(&Selector::parse(".result_text a").unwrap()).next() {
            title.inner_html()
        } else {
            log::info!("No results in Fn get_imdb_data for {title} {year} on {url_query}");
            return Err(Box::new(FwErrors::ZeroResults));
        };

        let title_id = if let Some(id) = document.select(&Selector::parse(".result_text").unwrap()).next() {
            let title_id = id.inner_html();
            let re = Regex::new(r"(\d{7,8})").unwrap();
            format!(
                "tt{:0>7}",
                re.captures(title_id.as_str()).unwrap().get(0).unwrap().as_str()
            )
        } else {
            log::info!("No results in Fn get_imdb_data for {title} {year} on {url_query}");
            return Err(Box::new(FwErrors::ZeroResults));
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
            Html::parse_document(&response)
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
            log::info!("Invalid duration in Fn get_imdb_data on {url} for {title} {year} source: {url_query}");
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

        let imdb_data = IMDbApiDetails {
            id: title_id,
            title: imdb_title,
            duration,
        };

        Ok(imdb_data)
    }

    pub fn export_csv(&self, files: &mut ExportFiles) {
        let title = &self.fw_title_pl;
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
    pub fn new() -> Result<Self, std::io::Error> {
        let write_header = |wtr| -> Writer<File> {
            let mut wtr: Writer<File> = csv::Writer::from_writer(wtr);
            wtr.write_record([
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

impl AlternateTitle {
    #[must_use]
    pub fn score_title(language: &str) -> u8 {
        if language.contains("USA") || language.contains("angielski") {
            10
        } else if language.contains("oryginalny") {
            9
        } else if language.contains("główny") {
            8
        } else if language.contains("alternatywna pisownia") {
            7
        } else if language.contains("inny tytuł") {
            6
        } else if language.contains("Polska") {
            5
        } else {
            u8::MIN
        }
    }

    pub fn fw_get_titles(url: &str, client: &Client) -> Result<PriorityQueue<Self, u8>, FwErrors> {
        let response = client.get(url).send().unwrap().text()?;
        let document = Html::parse_document(&response);
        let select_titles = Selector::parse(".filmTitlesSection__title").unwrap();
        let select_language = Selector::parse(".filmTitlesSection__desc").unwrap();
        let mut titles = PriorityQueue::new();
        document
            .select(&select_titles)
            .into_iter()
            .zip(document.select(&select_language))
            .for_each(|(title, language)| {
                let title = title.inner_html();
                let language = language.inner_html();
                let score = Self::score_title(&language);
                titles.push(Self { language, title }, score);
            });
        Ok(titles)
    }
}

impl Default for ExportFiles {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scraping_alternative_titles() {
        let client = Client::builder().user_agent(USER_AGENT).gzip(true).build().unwrap();
        let mut expected_titles = PriorityQueue::new();
        [
            ("South Park", "USA (Tytuł oryginalny)"),
            ("Městečko South Park", "Czechy"),
            (
                "South Park",
                "USA (Tytuł oryginalny) / Argentyna / Hiszpania / Francja / Węgry / Polska (tytuł telewizyjny)",
            ),
            ("Pietu parkas", "Litwa"),
            ("Miasteczko South Park", "Polska (tytuł główny)"),
            ("Mestečko South Park", "Słowacja"),
            ("Saut Park", "Serbia"),
        ]
        .iter()
        .for_each(|(title, language)| {
            expected_titles.push(
                AlternateTitle {
                    title: title.to_string(),
                    language: language.to_string(),
                },
                AlternateTitle::score_title(language),
            );
        });
        let alternate_titles = AlternateTitle::fw_get_titles(
            "https://www.filmweb.pl/serial/Miasteczko+South+Park-1997-94331/titles",
            &client,
        );

        assert_eq!(expected_titles.len(), alternate_titles.unwrap().len())
    }

    #[test]
    fn alternative_titles_priorityqueue_ordering() {
        let mut expected_titles = PriorityQueue::new();
        [
            ("Title", "USA"),
            ("Los Titulos", "tytuł oryginalny"),
            ("The Title", "tytuł główny"),
            ("Titles", "alternatywna pisownia"),
            ("Tytuł", "Polska"),
            ("Titulo", "Hiszpański"),
            ("标题", "Chiński"),
        ]
        .iter()
        .for_each(|(title, language)| {
            expected_titles.push(
                AlternateTitle {
                    title: title.to_string(),
                    language: language.to_string(),
                },
                AlternateTitle::score_title(language),
            );
        });
        assert_eq!("USA", expected_titles.pop().unwrap().0.language);
        assert_eq!("tytuł oryginalny", expected_titles.pop().unwrap().0.language);
        assert_eq!("tytuł główny", expected_titles.pop().unwrap().0.language);
    }
}
