//! One reading of a Postgres connection URL, for the engine and the gateway.
//!
//! Design: `docs/plans/2026-10-12-other-local-users-cannot-reach-postgres-or-devtools.md`.
//!
//! A URL names its server in one of two ways:
//!
//! - **TCP**: `postgres://user:pass@host:5432/db`.
//! - **Unix socket**: a directory, either as a `host` query parameter
//!   (`postgres:///db?host=/dir&user=u`) or percent-encoded in the authority
//!   (`postgres://u@%2Fdir/db`). sqlx reads both.
//!
//! libpq takes either from `PGHOST`, so [`PgUrl::env`] serves both shapes. The
//! password travels in `PGPASSWORD`, never in argv: a subprocess's argv is
//! persisted in tool-call payloads and shown in the steps UI.
//!
//! [`PgUrl::read_only_url`] renders the URL the engine hands its agents as
//! `LUCIDOS_DATABASE_URL` (ADR 0481).

use std::path::PathBuf;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The port libpq assumes when a URL names none.
pub const DEFAULT_PORT: u16 = 5432;

/// Where a URL's server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    Tcp(String),
    /// The directory holding `.s.PGSQL.<port>`.
    Socket(PathBuf),
}

/// A parsed Postgres connection URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgUrl {
    pub user: Option<String>,
    /// `Some("")` for an explicit empty password (`user:@host`), which libpq
    /// treats differently from no password at all.
    pub password: Option<String>,
    pub host: Host,
    pub port: u16,
    pub database: String,
    /// Every other query parameter, such as `sslmode` or `options`, as written:
    /// key and value still percent-encoded.
    pub params: Vec<(String, String)>,
}

impl PgUrl {
    /// Parse `url`. A refusal quotes the URL redacted, so it is safe to show.
    ///
    /// Query parameters `host`, `port`, `user` and `password` override the
    /// authority, as in libpq. Others, such as `sslmode`, land in `params`.
    pub fn parse(url: &str) -> Result<Self, BoxError> {
        let refuse = |why: &str| -> BoxError { format!("{why}: '{}'", redact(url)).into() };
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .ok_or_else(|| refuse("not a postgres URL"))?;
        let (userinfo, after_userinfo) = split_userinfo(rest);
        let (before_query, query) = match after_userinfo.split_once('?') {
            Some((before, query)) => (before, Some(query)),
            None => (after_userinfo, None),
        };
        let (host_port, path) = before_query
            .split_once('/')
            .ok_or_else(|| refuse("postgres URL has no database path"))?;
        let (mut user, mut password) = match userinfo {
            Some(info) => match info.split_once(':') {
                Some((u, p)) => (Some(decode(u)?), Some(decode(p)?)),
                None => (Some(decode(info)?), None),
            },
            None => (None, None),
        };
        let (host_text, port_text) = match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        };
        let mut host = decode(host_text)?;
        let mut port = port_text.map(|p| parse_port(p, url)).transpose()?;
        let mut params = Vec::new();
        for pair in query.into_iter().flat_map(|q| q.split('&')) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "host" => host = decode(value)?,
                "port" => port = Some(parse_port(&decode(value)?, url)?),
                "user" => user = Some(decode(value)?),
                "password" => password = Some(decode(value)?),
                "" => {}
                _ => params.push((key.to_string(), value.to_string())),
            }
        }
        let database = decode(path)?;
        if database.is_empty() {
            return Err(refuse("postgres URL has no database"));
        }
        let host = if host.is_empty() {
            return Err(refuse("postgres URL has no host"));
        } else if host.starts_with('/') {
            Host::Socket(PathBuf::from(host))
        } else {
            Host::Tcp(host)
        };
        Ok(Self {
            user,
            password,
            host,
            port: port.unwrap_or(DEFAULT_PORT),
            database,
            params,
        })
    }

    /// The libpq environment for this URL: `PGUSER`, `PGPASSWORD`, `PGHOST`,
    /// `PGPORT` and `PGDATABASE`, plus the variable for each parameter in
    /// [`PARAM_ENV`], such as `sslmode` as `PGSSLMODE`. A missing user or
    /// password is left out, so libpq applies its own default rather than an
    /// empty value.
    pub fn env(&self) -> Vec<(&'static str, String)> {
        let mut vars = Vec::with_capacity(5);
        if let Some(user) = &self.user {
            vars.push(("PGUSER", user.clone()));
        }
        if let Some(password) = &self.password {
            vars.push(("PGPASSWORD", password.clone()));
        }
        let host = match &self.host {
            Host::Tcp(host) => host.clone(),
            Host::Socket(dir) => dir.to_string_lossy().into_owned(),
        };
        vars.push(("PGHOST", host));
        vars.push(("PGPORT", self.port.to_string()));
        vars.push(("PGDATABASE", self.database.clone()));
        for (key, value) in &self.params {
            let Some((_, var)) = PARAM_ENV.iter().find(|(param, _)| param == key) else {
                continue;
            };
            if let Ok(value) = decode(value) {
                vars.push((var, value));
            }
        }
        vars
    }

    /// This URL with every transaction read-only unless it opts in with
    /// `BEGIN READ WRITE`. libpq, sqlx, psycopg and node-postgres all read it.
    ///
    /// A socket URL puts everything in the query, the shape the gateway hands
    /// a packaged engine: sqlx refuses an empty authority host. Every other
    /// parameter, such as `sslmode`, carries over, and an existing `options`
    /// gains the read-only flag at its end.
    pub fn read_only_url(&self) -> String {
        let enc = |text: &str| urlencoding::encode(text).into_owned();
        let database = enc(&self.database);
        let mut options = None;
        let mut tail = String::new();
        for (key, value) in &self.params {
            if key == "options" {
                options = Some(value.as_str());
            } else {
                tail.push_str(&format!("{key}={value}&"));
            }
        }
        tail.push_str(&match options {
            Some(existing) => format!("options={existing}%20{READ_ONLY_OPTION}"),
            None => format!("options={READ_ONLY_OPTION}"),
        });
        match &self.host {
            Host::Tcp(host) => {
                let userinfo = match (&self.user, &self.password) {
                    (Some(user), Some(password)) => format!("{}:{}@", enc(user), enc(password)),
                    (Some(user), None) => format!("{}@", enc(user)),
                    (None, Some(password)) => format!(":{}@", enc(password)),
                    (None, None) => String::new(),
                };
                format!(
                    "postgresql://{userinfo}{host}:{}/{database}?{tail}",
                    self.port
                )
            }
            Host::Socket(dir) => {
                let mut query = format!("host={}&port={}", enc(&dir.to_string_lossy()), self.port);
                if let Some(user) = &self.user {
                    query.push_str(&format!("&user={}", enc(user)));
                }
                if let Some(password) = &self.password {
                    query.push_str(&format!("&password={}", enc(password)));
                }
                format!("postgresql:///{database}?{query}&{tail}")
            }
        }
    }
}

/// The URL parameters libpq also reads from the environment, and the variable
/// for each. Dropping one would silently change the connection, such as
/// `sslmode=require` falling back to libpq's default `prefer`.
pub const PARAM_ENV: &[(&str, &str)] = &[
    ("sslmode", "PGSSLMODE"),
    ("sslrootcert", "PGSSLROOTCERT"),
    ("sslcert", "PGSSLCERT"),
    ("sslkey", "PGSSLKEY"),
    ("sslcrl", "PGSSLCRL"),
    ("options", "PGOPTIONS"),
    ("application_name", "PGAPPNAME"),
    ("connect_timeout", "PGCONNECT_TIMEOUT"),
    ("target_session_attrs", "PGTARGETSESSIONATTRS"),
];

/// The `options` flag that opens every session read-only, percent-encoded.
/// [`PgUrl::read_only_url`] always ends with it.
pub const READ_ONLY_OPTION: &str = "-c%20default_transaction_read_only%3Don";

/// `url` with its password hidden: the userinfo becomes `***@`, and a
/// `password=` query parameter becomes `password=***`.
///
/// Works on any string, parseable or not, so an error about a malformed URL
/// can still quote it. The userinfo is found as [`PgUrl::parse`] finds it.
pub fn redact(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let (userinfo, after_userinfo) = split_userinfo(rest);
    let authority = if userinfo.is_some() { "***@" } else { "" };
    let tail = match after_userinfo.split_once('?') {
        Some((path, query)) => {
            let query: Vec<&str> = query
                .split('&')
                .map(|pair| {
                    if pair.starts_with("password=") {
                        "password=***"
                    } else {
                        pair
                    }
                })
                .collect();
            format!("{path}?{}", query.join("&"))
        }
        None => after_userinfo.to_string(),
    };
    format!("{scheme}{authority}{tail}")
}

/// Split the URL after its scheme at the `@` that ends the userinfo.
///
/// That is the last `@` with a `/` after it, since the database path follows
/// the host. So an unencoded `/`, `?` or `@` in a password stays in the
/// userinfo. Neither the parser nor [`redact`] reads part of it as the host. A URL with no path falls back to the last `@` before any `/`, which
/// still leaves an `@` in the query alone.
fn split_userinfo(rest: &str) -> (Option<&str>, &str) {
    let ats = || rest.match_indices('@').map(|(i, _)| i);
    let first_slash = rest.find('/').unwrap_or(rest.len());
    let end = ats()
        .rfind(|&i| rest[i + 1..].contains('/'))
        .or_else(|| ats().rfind(|&i| i < first_slash));
    match end {
        Some(i) => (Some(&rest[..i]), &rest[i + 1..]),
        None => (None, rest),
    }
}

fn decode(text: &str) -> Result<String, BoxError> {
    Ok(urlencoding::decode(text)?.into_owned())
}

fn parse_port(text: &str, url: &str) -> Result<u16, BoxError> {
    text.parse::<u16>()
        .map_err(|_| format!("postgres URL has a bad port: '{}'", redact(url)).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(url: &str) -> std::collections::HashMap<&'static str, String> {
        PgUrl::parse(url).unwrap().env().into_iter().collect()
    }

    #[test]
    fn a_tcp_url_parses_whole() {
        assert_eq!(
            PgUrl::parse("postgres://lucidos:secret@127.0.0.1:5544/lucidos_dev?sslmode=disable")
                .unwrap(),
            PgUrl {
                user: Some("lucidos".into()),
                password: Some("secret".into()),
                host: Host::Tcp("127.0.0.1".into()),
                port: 5544,
                database: "lucidos_dev".into(),
                params: vec![("sslmode".into(), "disable".into())],
            }
        );
    }

    #[test]
    fn both_schemes_parse() {
        let url = PgUrl::parse("postgresql://u:p@h:1234/db").unwrap();
        assert_eq!(url.host, Host::Tcp("h".into()));
        assert_eq!(url.port, 1234);
    }

    #[test]
    fn a_missing_port_is_the_default() {
        assert_eq!(
            PgUrl::parse("postgres://u@host/db").unwrap().port,
            DEFAULT_PORT
        );
    }

    #[test]
    fn a_missing_password_is_none_and_an_empty_one_is_kept() {
        let none = env_map("postgres://lucidos@127.0.0.1:5599/lucidos");
        assert!(!none.contains_key("PGPASSWORD"));
        let empty = env_map("postgres://u:@host:5432/db");
        assert_eq!(empty.get("PGPASSWORD").map(String::as_str), Some(""));
    }

    #[test]
    fn userinfo_and_database_are_percent_decoded() {
        let url = PgUrl::parse("postgres://u%2Bx:p%40ss%2Fword@host:5432/my%20db").unwrap();
        assert_eq!(url.user.as_deref(), Some("u+x"));
        assert_eq!(url.password.as_deref(), Some("p@ss/word"));
        assert_eq!(url.database, "my db");
    }

    #[test]
    fn a_url_with_no_userinfo_leaves_the_user_to_libpq() {
        let env = env_map("postgres://localhost:5432/db");
        assert!(!env.contains_key("PGUSER"));
        assert_eq!(env.get("PGHOST").map(String::as_str), Some("localhost"));
    }

    /// The socket form the gateway hands a packaged engine. The directory can
    /// hold a space (`Application Support`) and an `@` (a home named after an
    /// email address).
    #[test]
    fn a_socket_url_in_the_query_parses_to_its_directory() {
        let url = PgUrl::parse(
            "postgres:///lucidos_myws?host=%2FUsers%2Fme.x%40example.com%2F.lucidos%2Frun%2Fpg%20x&user=lucidos",
        )
        .unwrap();
        assert_eq!(
            url.host,
            Host::Socket(PathBuf::from("/Users/me.x@example.com/.lucidos/run/pg x"))
        );
        assert_eq!(url.user.as_deref(), Some("lucidos"));
        assert_eq!(url.port, DEFAULT_PORT);
        assert_eq!(url.database, "lucidos_myws");
    }

    #[test]
    fn a_socket_url_in_the_authority_parses_to_its_directory() {
        let url = PgUrl::parse("postgres://lucidos@%2Ftmp%2Fpg:6000/db").unwrap();
        assert_eq!(url.host, Host::Socket(PathBuf::from("/tmp/pg")));
        assert_eq!(url.port, 6000);
    }

    #[test]
    fn a_socket_url_gives_libpq_its_directory_as_the_host() {
        let env = env_map("postgres:///db?host=/home/u/.lucidos/run/pg&port=5432&user=lucidos");
        assert_eq!(
            env.get("PGHOST").map(String::as_str),
            Some("/home/u/.lucidos/run/pg")
        );
        assert_eq!(env.get("PGPORT").map(String::as_str), Some("5432"));
        assert_eq!(env.get("PGUSER").map(String::as_str), Some("lucidos"));
        assert_eq!(env.get("PGDATABASE").map(String::as_str), Some("db"));
    }

    /// The rendered URL names the same server, user and database, with every
    /// parameter kept and the read-only flag added.
    fn assert_round_trips(source: &str, rendered: &str) {
        let original = PgUrl::parse(source).unwrap();
        let mut expected = original.params.clone();
        if !expected.iter().any(|(key, _)| key == "options") {
            expected.push(("options".into(), READ_ONLY_OPTION.into()));
        }
        let back = PgUrl::parse(rendered).unwrap();
        assert_eq!(
            PgUrl {
                params: Vec::new(),
                ..back.clone()
            },
            PgUrl {
                params: Vec::new(),
                ..original
            }
        );
        let keys = |params: &[(String, String)]| {
            let mut keys: Vec<String> = params.iter().map(|(k, _)| k.clone()).collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&back.params), keys(&expected));
        assert!(rendered.ends_with(READ_ONLY_OPTION), "{rendered}");
    }

    #[test]
    fn a_tcp_url_renders_read_only_and_keeps_its_parameters() {
        let source = "postgres://lucidos:p%40ss@127.0.0.1:5544/lucidos_dev?sslmode=require";
        let rendered = PgUrl::parse(source).unwrap().read_only_url();
        assert_eq!(
            rendered,
            "postgresql://lucidos:p%40ss@127.0.0.1:5544/lucidos_dev\
             ?sslmode=require&options=-c%20default_transaction_read_only%3Don"
        );
        assert_round_trips(source, &rendered);
    }

    /// The packaged engine's socket URL, with a space and an `@` in the
    /// directory.
    #[test]
    fn a_socket_url_renders_read_only_and_parses_back() {
        let source =
            "postgres:///lucidos_myws?host=%2FUsers%2Fme.x%40example.com%2Frun%2Fpg%20x&user=lucidos";
        let rendered = PgUrl::parse(source).unwrap().read_only_url();
        assert_eq!(
            rendered,
            "postgresql:///lucidos_myws\
             ?host=%2FUsers%2Fme.x%40example.com%2Frun%2Fpg%20x&port=5432&user=lucidos\
             &options=-c%20default_transaction_read_only%3Don"
        );
        assert_round_trips(source, &rendered);
    }

    #[test]
    fn existing_options_gain_the_read_only_flag_at_their_end() {
        let source = "postgres://u@h/db?options=-c%20search_path%3Dapp&application_name=x";
        let rendered = PgUrl::parse(source).unwrap().read_only_url();
        assert_eq!(
            rendered,
            "postgresql://u@h:5432/db?application_name=x\
             &options=-c%20search_path%3Dapp%20-c%20default_transaction_read_only%3Don"
        );
        assert_round_trips(source, &rendered);
    }

    #[test]
    fn a_url_with_no_userinfo_renders_none() {
        let url = PgUrl::parse("postgres://localhost/db").unwrap();
        assert_eq!(
            url.read_only_url(),
            "postgresql://localhost:5432/db?options=-c%20default_transaction_read_only%3Don"
        );
    }

    /// A backup of a server that insists on TLS must insist too.
    #[test]
    fn connection_parameters_reach_the_libpq_environment() {
        let env = env_map(
            "postgres://u@db.example:5432/x?sslmode=require&sslrootcert=%2Fetc%2Fca.pem\
             &options=-c%20search_path%3Dapp&unknown=1",
        );
        assert_eq!(env.get("PGSSLMODE").map(String::as_str), Some("require"));
        assert_eq!(
            env.get("PGSSLROOTCERT").map(String::as_str),
            Some("/etc/ca.pem")
        );
        assert_eq!(
            env.get("PGOPTIONS").map(String::as_str),
            Some("-c search_path=app")
        );
        assert_eq!(env.len(), 7, "an unknown parameter sets nothing: {env:?}");
    }

    /// An unencoded `/` or `@` in a password must not be read as the host.
    #[test]
    fn a_raw_slash_or_at_in_a_password_stays_in_the_userinfo() {
        let url = PgUrl::parse("postgres://u:pa/ss@word@db:5432/x").unwrap();
        assert_eq!(url.password.as_deref(), Some("pa/ss@word"));
        assert_eq!(url.host, Host::Tcp("db".into()));
        assert_eq!(
            redact("postgres://u:pa/ss@db:5432/x"),
            "postgres://***@db:5432/x"
        );
    }

    #[test]
    fn an_at_in_the_query_is_not_userinfo() {
        let url = PgUrl::parse("postgres://localhost/db?application_name=a@b").unwrap();
        assert_eq!(url.user, None);
        assert_eq!(url.host, Host::Tcp("localhost".into()));
        assert_eq!(url.database, "db");
    }

    #[test]
    fn query_parameters_override_the_authority() {
        let url = PgUrl::parse("postgres://a:b@h:1/db?user=c&password=d&host=k&port=2").unwrap();
        assert_eq!(url.user.as_deref(), Some("c"));
        assert_eq!(url.password.as_deref(), Some("d"));
        assert_eq!(url.host, Host::Tcp("k".into()));
        assert_eq!(url.port, 2);
    }

    #[test]
    fn malformed_urls_are_refused_without_their_password() {
        for bad in [
            "not a url at all",
            "http://wrong/scheme",
            "postgres://u:hunter2@host",
            "postgres://u:hunter2@/db",
            "postgres://u:hunter2@host:notaport/db",
            "postgres://u:hunter2@host:5432/",
            "postgres://u:hun/ter2@host:notaport/db",
            "postgres://u:hun?ter2@host:notaport/db",
        ] {
            let err = PgUrl::parse(bad).unwrap_err().to_string();
            assert!(!err.contains("hunter2"), "{bad} leaked its password: {err}");
        }
    }

    /// Every message built from a database URL goes through the redactor, and
    /// those messages reach the boot splash and the picker's tooltip.
    #[test]
    fn a_redacted_url_keeps_its_shape_and_drops_its_password() {
        let redacted = redact("postgres://lucidos:secret@127.0.0.1:5544/lucidos_dev");
        assert_eq!(redacted, "postgres://***@127.0.0.1:5544/lucidos_dev");
        // A password containing an `@` is removed whole.
        assert_eq!(
            redact("postgres://u:p@ss@db.example:5432/x?sslmode=require"),
            "postgres://***@db.example:5432/x?sslmode=require"
        );
        assert_eq!(
            redact("postgres:///db?host=/run/pg&password=secret&user=u"),
            "postgres:///db?host=/run/pg&password=***&user=u"
        );
        // Nothing to hide, and a malformed value, both pass through readable.
        assert_eq!(
            redact("postgres://127.0.0.1:5432/lucidos"),
            "postgres://127.0.0.1:5432/lucidos"
        );
        assert_eq!(redact("not-a-url"), "not-a-url");
    }
}
