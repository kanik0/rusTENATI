use reqwest::Client;
use scraper::{Html, Selector};
use tracing::{debug, warn};

/// Check if an HTTP response body looks like an AWS WAF challenge page.
pub fn is_waf_challenge(status: u16, body: &str) -> bool {
    if status != 403 && status != 202 {
        return false;
    }

    body.contains("aws-waf-token")
        || body.contains("awswaf")
        || body.contains("captcha.js")
        || (body.contains("challenge") && body.contains("aws"))
}

/// Extract challenge URL and hidden form fields from WAF page.
/// Returns (action_url, form_fields) or None.
fn extract_challenge_data(body: &str) -> Option<(String, Vec<(String, String)>)> {
    let document = Html::parse_document(body);

    // Look for form action URL
    let form_sel = Selector::parse("form[action]").ok()?;
    let action = document
        .select(&form_sel)
        .next()?
        .value()
        .attr("action")?
        .to_string();

    // Extract hidden form fields
    let input_sel = Selector::parse("input[type='hidden']").unwrap();
    let fields: Vec<(String, String)> = document
        .select(&input_sel)
        .filter_map(|input| {
            let name = input.value().attr("name")?.to_string();
            let value = input.value().attr("value").unwrap_or_default().to_string();
            Some((name, value))
        })
        .collect();

    Some((action, fields))
}

/// Try to solve a simple WAF challenge (non-CAPTCHA).
///
/// Some WAF challenges just require submitting a token back. CAPTCHAs cannot
/// be solved automatically and will return an error.
pub async fn try_solve_challenge(
    client: &Client,
    _challenge_url: &str,
    body: &str,
) -> Result<bool, reqwest::Error> {
    // Extract data synchronously (Html is not Send)
    let challenge_data = extract_challenge_data(body);

    let (action_url, form_data) = match challenge_data {
        Some(data) if !data.1.is_empty() => data,
        _ => {
            warn!("WAF challenge has no form data to submit - may require CAPTCHA");
            return Ok(false);
        }
    };

    debug!(
        "Submitting WAF challenge to {} with {} fields",
        action_url,
        form_data.len()
    );

    let response = client
        .post(&action_url)
        .form(&form_data)
        .send()
        .await?;

    let solved = response.status().is_success() || response.status().is_redirection();

    if solved {
        debug!("WAF challenge solved successfully");
    } else {
        warn!(
            "WAF challenge submission returned status {}",
            response.status()
        );
    }

    Ok(solved)
}
