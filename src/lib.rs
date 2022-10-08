use csv::Writer;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, fs, fs::File};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux i686; rv:101.0) Gecko/20100101 Firefox/101.0";

// TODO: use thiserror or anyhow idk, whatever is suitable for a lib
#[derive(Debug)]
pub enum FwErrors {
    ZeroResults,
    InvalidDuration,
    InvalidJwt,
}

impl Error for FwErrors {}

impl fmt::Display for FwErrors {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error occured") // it will change soon, dont worry
    }
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
    username: String,
    token: String,
    session: String,
    jwt: String,
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
    pub const fn new(username: String, token: String, session: String, jwt: String) -> Self {
        Self {
            username,
            token,
            session,
            jwt,
        }
    }

    pub fn get_counts(&self, fw_client: &Client) -> Result<(u16, u16, u16), Box<dyn std::error::Error>> {
        let user_source = fw_client
            .get(format!("https://www.filmweb.pl/user/{}", self.username))
            .send()
            .unwrap()
            .text()
            .unwrap();
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

    pub fn scrape_from_page(&mut self, fw_client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        let html = Html::parse_document(&self.page_source);
        for votebox in html.select(&Selector::parse("div.myVoteBox").unwrap()) {
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
            let api_response = match self.page_type {
                FwPageNumber::Films(_) => Some(
                    fw_client
                        .get(format!(
                            "https://www.filmweb.pl/api/v1/logged/vote/film/{}/details",
                            title_id
                        ))
                        .send(),
                ),
                FwPageNumber::Serials(_) => Some(
                    fw_client
                        .get(format!(
                            "https://www.filmweb.pl/api/v1/logged/vote/serial/{}/details",
                            title_id
                        ))
                        .send(),
                ),
                FwPageNumber::WantsToSee(_) => None,
            };

            // JWT could be invalidated meanwhile
            let rating: Option<FwApiDetails> = match api_response {
                Some(response) => match response?.json() {
                    Ok(v) => v,
                    Err(_) => return Err(Box::new(FwErrors::InvalidJwt)),
                },
                None => None,
            };

            // Parse year properly, set it to Year::Range if year is in a format for example, 2015-2019
            // It's used in serials mostly
            let year = if year.contains('-') {
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
                Year::OneYear(year.trim().parse::<u16>()?)
            };

            let duration = {
                let response = fw_client.get(&url).send()?.text()?;
                let response = Html::parse_document(response.as_str());
                match response
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
                fw_title_id: title_id.trim().parse::<u32>()?,
                fw_title_pl: title_pl,
                fw_title_orig: title,
                title_type: self.page_type.into(),
                fw_duration: duration,
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
                        Err(_) => None,
                    },
                },
            },
            None => match self.get_imdb_data_advanced(&self.fw_title_pl, year, year, imdb_client) {
                Ok(api) => Some(api),
                Err(_) => match self.get_imdb_data(&self.fw_title_pl, year, imdb_client) {
                    Ok(api) => Some(api),
                    Err(_) => None,
                },
            },
        };
    }

    pub fn get_imdb_data_advanced(
        &self,
        title: &str,
        year_start: u16,
        year_end: u16,
        imdb_client: &Client,
    ) -> Result<IMDbApiDetails, Box<FwErrors>> {
        let url = format!(
            "https://www.imdb.com/search/title/?title={}&release_date={},{}&adult=include",
            title, year_start, year_end
        );
        let response = imdb_client.get(url).send().unwrap().text().unwrap();
        let response = Html::parse_document(response.as_str());
        let title_data = match response
            .select(&Selector::parse("div.lister-item-image").unwrap())
            .next()
        {
            Some(id) => id,
            None => return Err(Box::new(FwErrors::ZeroResults)),
        };
        let title_id = title_data.inner_html();
        let imdb_title = response
            .select(&Selector::parse("img.loadlate").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("alt")
            .unwrap();
        let re = Regex::new(r"(\d{7,8})").unwrap();
        let title_id = format!(
            "{:08}",
            re.captures(title_id.as_str()).unwrap().get(0).unwrap().as_str()
        );

        let duration = {
            let x = match response.select(&Selector::parse(".runtime").unwrap()).next() {
                Some(a) => a.inner_html().replace(" min", ""),
                None => return Err(Box::new(FwErrors::InvalidDuration)),
            };
            match x.parse::<u32>() {
                Ok(x) => x,
                Err(_) => return Err(Box::new(FwErrors::InvalidDuration)),
            }
        };
        let imdb_data = IMDbApiDetails {
            id: title_id.trim().parse::<u32>().unwrap().to_string(),
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
        let url = format!("https://www.imdb.com/find?q={}+{}", title, year);
        let response = imdb_client.get(url).send().unwrap().text().unwrap();
        let response_parsed = Html::parse_document(response.as_str());
        let imdb_title = match response_parsed
            .select(&Selector::parse(".result_text a").unwrap())
            .next()
        {
            None => return Err(Box::new(FwErrors::ZeroResults)),
            Some(title) => title.inner_html(),
        };
        let title_id = match response_parsed.select(&Selector::parse(".result_text").unwrap()).next() {
            Some(id) => id,
            None => return Err(Box::new(FwErrors::ZeroResults)),
        };
        // get url of a title, and grab the duration
        let url_suffix = response_parsed
            .select(&Selector::parse("td.result_text a").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("href")
            .unwrap();
        let full_url = format!("https://www.imdb.com{}", url_suffix);
        println!("{full_url}");
        let response = imdb_client.get(full_url).send().unwrap().text().unwrap();
        let response_parsed = Html::parse_document(response.as_str());
        println!("{title}, {year}");

        let get_duration = |nth| {
            response_parsed
                .select(&Selector::parse(".ipc-inline-list__item").unwrap())
                .nth(nth)
                .expect("Panic occured while trying to export {title} {year}")
                .inner_html()
        };

        let mut dirty_duration = get_duration(5);
        if dirty_duration.contains("Unrated") || dirty_duration.contains("Not Rated") || dirty_duration.contains("TV") {
            dirty_duration = get_duration(6);
        }

        if dirty_duration.len() > 40 {
            return Err(Box::new(FwErrors::InvalidDuration));
        }

        println!("{title}");
        let duration = {
            let mut duration;
            let mut dirty_duration = dirty_duration.replace("<!---->", "");
            if dirty_duration.contains('h') {
                let mut dirty_duration = dirty_duration.replace(' ', "");
                println!("{dirty_duration}");
                duration = dirty_duration
                    .chars()
                    .next()
                    .expect("Handling duration failed")
                    .to_digit(10)
                    .expect("Conversion to int failed")
                    * 60;
                dirty_duration.remove(0);
                dirty_duration.retain(|c| c.is_ascii_digit());
                duration += dirty_duration.parse::<u32>().unwrap_or(0);
            } else {
                dirty_duration.retain(|c| c.is_ascii_digit());
                duration = dirty_duration.parse::<u32>().unwrap();
            }
            duration
        };

        println!("{duration}");

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

        let mut fields = ["", "", "", "", "", "", "", "", "", "", "", "", ""];
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

pub fn filmweb_client_builder(user: &FwUser) -> Result<Client, reqwest::Error> {
    let cookies = format!(
        "_fwuser_token={}; _fwuser_sessionId={}; JWT={};",
        user.token, user.session, user.jwt
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

pub fn imdb_client_builder() -> Result<Client, reqwest::Error> {
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
        if let Err(e) = fs::create_dir("./exports") {
            match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => panic!("{}", e),
            }
        };
        let generic = File::create("exports/generic.csv").unwrap();
        let want2see = File::create("exports/want2see.csv").unwrap();
        let favorited = File::create("exports/favorited.csv").unwrap();
        let generic = write_header(generic);
        let want2see = write_header(want2see);
        let favorited = write_header(favorited);
        Self {
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
