use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PrivacyRejection {
    #[error("credential or authentication secret detected")]
    Credential,
    #[error("payment-card or bank-account number detected")]
    FinancialIdentifier,
    #[error("private key material detected")]
    PrivateKey,
    #[error("password-field content cannot become memory")]
    PasswordField,
    #[error("sensitive personal trait cannot be inferred")]
    SensitiveTraitInference,
    #[error("speculative judgment about another person cannot be stored")]
    SpeculativePersonJudgment,
}

/// Conservatively rejects classes of content that may never be auto-stored.
///
/// This is a final deterministic guard, not a general-purpose DLP scanner.
pub fn inspect_private_content(
    content: &str,
    is_password_field: bool,
    is_inferred: bool,
) -> Result<(), PrivacyRejection> {
    if is_password_field {
        return Err(PrivacyRejection::PasswordField);
    }

    let lower = content.to_ascii_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key-----") {
        return Err(PrivacyRejection::PrivateKey);
    }

    const SECRET_MARKERS: &[&str] = &[
        "password:",
        "password is",
        "api key:",
        "api_key=",
        "access token:",
        "bearer ",
        "recovery code:",
        "one-time code:",
        "otp:",
        "authentication code:",
    ];
    if SECRET_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Err(PrivacyRejection::Credential);
    }

    if digit_runs(content).any(|digits| (13..=19).contains(&digits.len()) && luhn_valid(&digits)) {
        return Err(PrivacyRejection::FinancialIdentifier);
    }

    if is_inferred {
        const SENSITIVE_TRAITS: &[&str] = &[
            "diagnosed with",
            "sexual orientation",
            "is gay",
            "is lesbian",
            "religion is",
            "is muslim",
            "is christian",
            "is hindu",
            "political affiliation",
            "is a democrat",
            "is a republican",
        ];
        if SENSITIVE_TRAITS.iter().any(|marker| lower.contains(marker)) {
            return Err(PrivacyRejection::SensitiveTraitInference);
        }
        const JUDGMENTS: &[&str] = &[
            "is untrustworthy",
            "cannot be trusted",
            "has bad intentions",
            "is manipulative",
            "has a personality disorder",
        ];
        if JUDGMENTS.iter().any(|marker| lower.contains(marker)) {
            return Err(PrivacyRejection::SpeculativePersonJudgment);
        }
    }
    Ok(())
}

fn digit_runs(input: &str) -> impl Iterator<Item = String> + '_ {
    input
        .split(|c: char| !c.is_ascii_digit() && c != ' ' && c != '-')
        .map(|part| part.chars().filter(char::is_ascii_digit).collect())
        .filter(|digits: &String| !digits.is_empty())
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0;
    for (index, byte) in digits.bytes().rev().enumerate() {
        let mut digit = u32::from(byte - b'0');
        if index % 2 == 1 {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
    }
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_keys_credentials_and_cards() {
        assert_eq!(
            inspect_private_content("-----BEGIN RSA PRIVATE KEY----- abc", false, false),
            Err(PrivacyRejection::PrivateKey)
        );
        assert_eq!(
            inspect_private_content("My password is hunter2", false, false),
            Err(PrivacyRejection::Credential)
        );
        assert_eq!(
            inspect_private_content("Card 4111 1111 1111 1111", false, false),
            Err(PrivacyRejection::FinancialIdentifier)
        );
    }

    #[test]
    fn ordinary_numbers_and_explicit_traits_are_not_over_rejected() {
        assert!(inspect_private_content("Budget is 123456", false, false).is_ok());
        assert!(inspect_private_content("I am Hindu", false, false).is_ok());
    }
}
