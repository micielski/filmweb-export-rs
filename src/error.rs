use thiserror::Error;

#[derive(Error, Debug)]
pub enum FwErrors {
    #[error("title not found")]
    ZeroResults,
    #[error("couldn't fetch duration")]
    InvalidDuration,
    #[error("provided JWT is invalid / has invalidated, try again with a new one")]
    InvalidJwt,
    #[error("while parsing a year for title_id {}, string that caused that error: {}", .title_id, .failed_year)]
    InvalidYear { title_id: u32, failed_year: String },
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("while sending a request / building a client / parsing a response: {}", .source)]
    ReqwestError {
        #[from]
        source: reqwest::Error,
    },
    #[error("while inserting a cookie to a header: {}", .source)]
    InvalidHeaderValue {
        #[from]
        source: reqwest::header::InvalidHeaderValue,
    },
    #[error("while probably trying to convert an id string to int: {}", .source)]
    InvalidId {
        #[from]
        source: std::num::ParseIntError,
    },
}
