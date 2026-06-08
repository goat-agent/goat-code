use goat_provider::{AuthMethod, Model};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinHandle};

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_mins(5))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

pub fn authenticated(auth: AuthMethod, bearer: &Option<String>) -> bool {
    match auth {
        AuthMethod::None => true,
        _ => bearer.is_some(),
    }
}

pub fn validate_bearer(
    client: reqwest::Client,
    url: String,
    auth: AuthMethod,
    bearer: Option<String>,
) -> JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        if matches!(auth, AuthMethod::None) {
            return Ok(());
        }
        let Some(token) = bearer else {
            return Err("no credentials".to_owned());
        };
        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "could not reach provider".to_owned())?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            Err("invalid credentials".to_owned())
        } else {
            Err(format!("could not reach provider: {status}"))
        }
    })
}

pub fn discover_models(
    client: reqwest::Client,
    url: String,
    bearer: Option<String>,
    filter: Option<fn(&str) -> bool>,
    tx: mpsc::Sender<Model>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut builder = client.get(&url);
        if let Some(token) = &bearer {
            builder = builder.bearer_auth(token);
        }
        let Ok(resp) = builder.send().await else {
            return;
        };
        let Ok(models) = resp.json::<ModelsResponse>().await else {
            return;
        };
        for model in models.data {
            if let Some(keep) = filter
                && !keep(&model.id)
            {
                continue;
            }
            if tx.send(Model { id: model.id }).await.is_err() {
                return;
            }
        }
    })
}

#[derive(Deserialize)]
pub(crate) struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelDto>,
}

#[derive(Deserialize)]
pub(crate) struct ModelDto {
    pub id: String,
}
