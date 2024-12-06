use rusqlite::Error as SQLError;
use std::fmt::Display;
use std::io::{Error as IOError, ErrorKind};

#[derive(Debug)]
pub enum DatabaseError {
    NotFound(String),
    AlreadyPresent(String),
    IOError(ErrorKind),
    SQLError(String),
    SQLPoolError(String),
}

impl From<IOError> for DatabaseError {
    fn from(error: IOError) -> DatabaseError {
        DatabaseError::IOError(error.kind())
    }
}

impl From<SQLError> for DatabaseError {
    fn from(error: SQLError) -> DatabaseError {
        DatabaseError::SQLError(error.to_string())
    }
}

impl From<r2d2::Error> for DatabaseError {
    fn from(error: r2d2::Error) -> Self {
        // Use `Display` impl to print "timed out waiting for connection"
        DatabaseError::SQLPoolError(format!("{}", error))
    }
}

impl Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
