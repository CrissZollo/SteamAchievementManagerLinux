use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    /// No Steam installation was found in any of the known locations.
    SteamNotFound,
    /// The Steam root exists but does not contain a usable `steamclient.so`.
    ClientLibraryNotFound {
        searched: Vec<PathBuf>,
    },
    /// `dlopen` failed.
    LoadLibrary {
        path: PathBuf,
        source: String,
    },
    /// A required export was absent from `steamclient.so`.
    MissingExport(&'static str),
    /// `CreateInterface` returned null for this version string.
    CreateInterface(&'static str),
    /// `CreateSteamPipe` returned 0.
    CreateSteamPipe,
    /// `ConnectToGlobalUser` returned 0. Almost always means Steam is not
    /// running, or the account is busy in a Family Share session.
    ConnectToGlobalUser,
    /// An interface getter on ISteamClient returned null.
    GetInterface(&'static str),
    /// The app ID reported by Steam did not match the one requested.
    AppIdMismatch {
        requested: u32,
        actual: u32,
    },
    /// The vtable does not look like what we expect, so calling through it
    /// would execute arbitrary functions. Refuse rather than corrupt data.
    VtableSanityCheckFailed(String),
    Io(std::io::Error),
    Vdf(sam_vdf::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::SteamNotFound => write!(
                f,
                "no Steam installation found (looked in ~/.steam/steam, \
                 ~/.local/share/Steam and the Flatpak location)"
            ),
            Error::ClientLibraryNotFound { searched } => {
                write!(f, "steamclient.so not found; looked in: ")?;
                for (i, p) in searched.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p.display())?;
                }
                Ok(())
            }
            Error::LoadLibrary { path, source } => {
                write!(f, "failed to load {}: {source}", path.display())
            }
            Error::MissingExport(name) => {
                write!(f, "steamclient.so does not export {name}")
            }
            Error::CreateInterface(v) => {
                write!(f, "CreateInterface returned null for {v}")
            }
            Error::CreateSteamPipe => write!(f, "failed to create a Steam pipe"),
            Error::ConnectToGlobalUser => write!(
                f,
                "failed to connect to the global Steam user. Start Steam and sign in, \
                 then try again. If the account is in a Family Share session elsewhere, \
                 the library may be locked."
            ),
            Error::GetInterface(name) => write!(f, "Steam returned no {name} interface"),
            Error::AppIdMismatch { requested, actual } => write!(
                f,
                "app ID mismatch: asked for {requested}, Steam reports {actual}"
            ),
            Error::VtableSanityCheckFailed(detail) => write!(
                f,
                "steamclient.so vtable layout is not what this build expects ({detail}). \
                 Refusing to continue, because calling through a mismatched vtable can \
                 corrupt your stats."
            ),
            Error::Io(e) => write!(f, "{e}"),
            Error::Vdf(e) => write!(f, "schema parse error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Vdf(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<sam_vdf::Error> for Error {
    fn from(e: sam_vdf::Error) -> Self {
        Error::Vdf(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
