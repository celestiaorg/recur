//! ID-token verification against the published OpenID issuer and JWKS. Merely
//! decoding a JWT does not establish which account authorized a sign-in.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::Auth;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct IdentityClaims {
    #[serde(rename = "sub")]
    pub subject: String,
    pub email: String,
    #[serde(rename = "iss")]
    pub issuer: String,
    #[serde(rename = "aud")]
    pub audience: Value,
    #[serde(rename = "exp")]
    pub expires: i64,
    #[serde(rename = "chatgpt_account_id")]
    pub workspace_id: String,
    #[serde(rename = "https://api.openai.com/auth")]
    pub auth: AuthClaim,
    pub organizations: Vec<Organization>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthClaim {
    #[serde(rename = "chatgpt_account_id")]
    pub workspace_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Organization {
    pub id: String,
}

impl IdentityClaims {
    /// The ChatGPT account (workspace) the token routes to.
    pub fn account_id(&self) -> String {
        [&self.workspace_id, &self.auth.workspace_id]
            .into_iter()
            .find(|s| !s.is_empty())
            .cloned()
            .or_else(|| self.organizations.first().map(|o| o.id.clone()))
            .unwrap_or_default()
    }

    /// The claims of any JWT without verification (an access token that came
    /// straight from the token endpoint).
    pub fn unverified(token: &str) -> Option<Self> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).ok()?).ok()
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Jwk {
    kid: String,
    kty: String,
    alg: String,
    #[serde(rename = "use")]
    usage: String,
    n: String,
    e: String,
}

impl Auth {
    pub(crate) async fn validate_id_token(
        &self,
        token: &str,
        client_id: &str,
    ) -> Result<IdentityClaims, String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err("invalid ChatGPT ID token".into());
        }
        let header = URL_SAFE_NO_PAD
            .decode(parts[0])
            .map_err(|_| "invalid ChatGPT ID token header".to_owned())?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Header {
            alg: String,
            kid: String,
        }
        let header: Header = serde_json::from_slice(&header).unwrap_or_default();
        if header.alg != "RS256" || header.kid.is_empty() {
            return Err("unsupported ChatGPT ID token signing key".into());
        }
        let raw = self.request(self.http.get(&self.jwks_url)).await?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        let jwks: Jwks =
            serde_json::from_slice(&raw).map_err(|_| "invalid OpenAI signing keys".to_owned())?;
        let key = jwks.keys.iter().find_map(|k| {
            if k.kid != header.kid
                || k.kty != "RSA"
                || !(k.alg.is_empty() || k.alg == "RS256")
                || !(k.usage.is_empty() || k.usage == "sig")
            {
                return None;
            }
            let n = URL_SAFE_NO_PAD.decode(&k.n).ok()?;
            let e = URL_SAFE_NO_PAD.decode(&k.e).ok()?;
            if e.is_empty() || e.len() > 4 {
                return None;
            }
            let exponent = e.iter().fold(0i64, |acc, b| acc << 8 | i64::from(*b));
            if !(3..=i64::from(i32::MAX)).contains(&exponent) {
                return None;
            }
            let strip = |b: Vec<u8>| b.into_iter().skip_while(|&x| x == 0).collect::<Vec<u8>>();
            Some((strip(n), strip(e)))
        });
        let Some((n, e)) = key else {
            return Err("ChatGPT ID token signing key not found".into());
        };
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| "invalid ChatGPT ID token signature".to_owned())?;
        let signed = format!("{}.{}", parts[0], parts[1]);
        aws_lc_rs::signature::RsaPublicKeyComponents { n: &n, e: &e }
            .verify(
                &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &signature,
            )
            .map_err(|_| "ChatGPT ID token signature verification failed".to_owned())?;
        let claims: IdentityClaims = URL_SAFE_NO_PAD
            .decode(parts[1])
            .ok()
            .and_then(|p| serde_json::from_slice(&p).ok())
            .ok_or_else(|| "invalid ChatGPT ID token claims".to_owned())?;
        let matched = match &claims.audience {
            Value::String(one) => one == client_id,
            Value::Array(all) => all.iter().any(|a| a.as_str() == Some(client_id)),
            _ => false,
        };
        if !matched
            || claims.issuer != self.issuer
            || claims.subject.is_empty()
            || claims.expires <= chrono::Utc::now().timestamp()
        {
            return Err("ChatGPT ID token identity validation failed".into());
        }
        Ok(claims)
    }
}
