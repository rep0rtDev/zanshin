//! SAML 2.0 service provider for the admin console: SP-initiated login (HTTP-Redirect
//! AuthnRequest → HTTP-POST Response), XML-DSig verification (exclusive C14N, RSA-SHA256 /
//! optionally RSA-SHA1) without libxml, assertion validity checks and attribute mapping.

use std::{collections::BTreeMap, io::Write, time::Duration};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use roxmltree::{Document, Node};
use rsa::{
    pkcs1::DecodeRsaPublicKey,
    pkcs1v15::{Signature, VerifyingKey},
    signature::Verifier,
    RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use zanshin_core::{Error, Result};

use super::oidc::ConsoleUser;
use crate::config::SamlConfig;

pub const STATE_COOKIE: &str = "zanshin_saml_state";
const STATE_TTL: Duration = Duration::from_secs(600);
const CLOCK_SKEW_SECS: i64 = 120;

const NS_SAMLP: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const NS_SAML: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const NS_DS: &str = "http://www.w3.org/2000/09/xmldsig#";
const NS_MD: &str = "urn:oasis:names:tc:SAML:2.0:metadata";
const ALG_RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const ALG_RSA_SHA1: &str = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
const DIGEST_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const DIGEST_SHA1: &str = "http://www.w3.org/2000/09/xmldsig#sha1";
const TRANSFORM_ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";

#[derive(Serialize, Deserialize)]
struct StateClaims {
    id: String,
    exp: i64,
}

#[derive(Clone)]
pub struct SamlService {
    cfg: Option<SamlConfig>,
    enc: EncodingKey,
    dec: DecodingKey,
    certs: Vec<RsaPublicKey>,
}

pub struct LoginStart {
    pub redirect_url: String,
    pub state_cookie: String,
}

/// Verified identity extracted from an assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub name_id: String,
    pub attributes: BTreeMap<String, Vec<String>>,
    pub issuer: String,
    pub in_response_to: Option<String>,
    pub session_index: Option<String>,
}

impl SamlService {
    pub fn new(cfg: Option<SamlConfig>, jwt_secret: &str) -> Self {
        let certs = cfg
            .as_ref()
            .map(|c| {
                c.idp_certs
                    .iter()
                    .filter_map(|pem| match parse_cert(pem) {
                        Ok(k) => Some(k),
                        Err(e) => {
                            tracing::error!(error = %e, "saml: unusable idp certificate");
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            cfg,
            enc: EncodingKey::from_secret(jwt_secret.as_bytes()),
            dec: DecodingKey::from_secret(jwt_secret.as_bytes()),
            certs,
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.is_some() && !self.certs.is_empty()
    }

    fn cfg(&self) -> Result<&SamlConfig> {
        self.cfg
            .as_ref()
            .ok_or_else(|| Error::unavailable("saml login is not configured"))
    }

    pub fn entity_id(&self) -> String {
        self.cfg
            .as_ref()
            .map(|c| {
                if c.sp_entity_id.is_empty() {
                    c.acs_url.trim_end_matches("/acs").to_string() + "/metadata"
                } else {
                    c.sp_entity_id.clone()
                }
            })
            .unwrap_or_default()
    }

    /// SP metadata document for registering Zanshin at the IdP.
    pub fn metadata(&self) -> Result<String> {
        let cfg = self.cfg()?;
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="{NS_MD}" entityID="{}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="{NS_SAMLP}">
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="{}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>
"#,
            xml_escape(&self.entity_id()),
            xml_escape(&cfg.acs_url)
        ))
    }

    /// Builds the AuthnRequest redirect and the state cookie that pins its ID.
    pub fn begin_login(&self) -> Result<LoginStart> {
        let cfg = self.cfg()?;
        let id = format!("_{}", random_token());
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let xml = format!(
            r#"<samlp:AuthnRequest xmlns:samlp="{NS_SAMLP}" xmlns:saml="{NS_SAML}" ID="{id}" Version="2.0" IssueInstant="{now}" Destination="{}" ProtocolBinding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" AssertionConsumerServiceURL="{}"><saml:Issuer>{}</saml:Issuer><samlp:NameIDPolicy Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress" AllowCreate="true"/></samlp:AuthnRequest>"#,
            xml_escape(&cfg.idp_sso_url),
            xml_escape(&cfg.acs_url),
            xml_escape(&self.entity_id())
        );
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(xml.as_bytes())
            .and_then(|_| enc.finish())
            .map(|deflated| {
                let b64 = STANDARD.encode(deflated);
                let mut url = url::Url::parse(&cfg.idp_sso_url)
                    .map_err(|e| Error::invalid(format!("saml idp_sso_url: {e}")))?;
                url.query_pairs_mut()
                    .append_pair("SAMLRequest", &b64)
                    .append_pair("RelayState", &id);
                Ok::<_, Error>(url.to_string())
            })
            .map_err(|e| Error::internal(anyhow::anyhow!("saml deflate: {e}")))?
            .map(|redirect_url| {
                let exp = Utc::now().timestamp() + STATE_TTL.as_secs() as i64;
                let state_cookie = jsonwebtoken::encode(
                    &Header::new(Algorithm::HS256),
                    &StateClaims { id, exp },
                    &self.enc,
                )
                .map_err(|e| Error::internal(anyhow::anyhow!("sign saml state: {e}")))?;
                Ok(LoginStart {
                    redirect_url,
                    state_cookie,
                })
            })?
    }

    /// Verifies a base64 `SAMLResponse` and maps it to a console user.
    pub fn finish_login(
        &self,
        saml_response_b64: &str,
        state_cookie: Option<&str>,
    ) -> Result<ConsoleUser> {
        let cfg = self.cfg()?;
        let expected_id = state_cookie.and_then(|c| {
            let mut v = Validation::new(Algorithm::HS256);
            v.validate_exp = true;
            jsonwebtoken::decode::<StateClaims>(c, &self.dec, &v)
                .ok()
                .map(|d| d.claims.id)
        });
        let xml = STANDARD
            .decode(saml_response_b64.trim())
            .map_err(|_| Error::unauthenticated("SAMLResponse is not base64"))?;
        let xml = String::from_utf8(xml)
            .map_err(|_| Error::unauthenticated("SAMLResponse is not UTF-8"))?;
        let a = verify_response(
            &xml,
            &self.certs,
            VerifyOptions {
                acs_url: &cfg.acs_url,
                audience: &self.entity_id(),
                issuer: if cfg.idp_entity_id.is_empty() {
                    None
                } else {
                    Some(&cfg.idp_entity_id)
                },
                in_response_to: expected_id.as_deref(),
                allow_sha1: cfg.allow_sha1,
                now: Utc::now(),
            },
        )?;
        let attr = |name: &str| {
            a.attributes
                .iter()
                .find(|(k, _)| {
                    k.eq_ignore_ascii_case(name)
                        || k.rsplit('/')
                            .next()
                            .is_some_and(|t| t.eq_ignore_ascii_case(name))
                })
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let mut email = attr(&cfg.email_attribute)
            .into_iter()
            .next()
            .unwrap_or_default();
        if email.is_empty() {
            email = attr("mail").into_iter().next().unwrap_or_default();
        }
        if email.is_empty() && a.name_id.contains('@') {
            email = a.name_id.clone();
        }
        let email = email.trim().to_lowercase();
        if !cfg.allowed_domains.is_empty() {
            let domain = email.rsplit('@').next().unwrap_or("");
            if email.is_empty()
                || !cfg
                    .allowed_domains
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(domain))
            {
                return Err(Error::denied("email domain is not allowed"));
            }
        }
        let roles = attr(&cfg.role_attribute);
        let role = roles
            .iter()
            .filter_map(|v| cfg.role_map.get(v).copied())
            .max()
            .or(cfg.default_role)
            .ok_or_else(|| Error::denied("no console role granted to this account"))?;
        let name = attr(&cfg.name_attribute)
            .into_iter()
            .next()
            .or_else(|| {
                let first = attr("givenName").into_iter().next();
                let last = attr("surname")
                    .into_iter()
                    .next()
                    .or_else(|| attr("sn").into_iter().next());
                match (first, last) {
                    (Some(f), Some(l)) => Some(format!("{f} {l}")),
                    (Some(f), None) => Some(f),
                    (None, Some(l)) => Some(l),
                    _ => None,
                }
            })
            .unwrap_or_else(|| email.clone());
        Ok(ConsoleUser {
            sub: if a.name_id.is_empty() {
                email.clone()
            } else {
                a.name_id.clone()
            },
            email,
            name,
            role,
        })
    }
}

pub struct VerifyOptions<'a> {
    pub acs_url: &'a str,
    pub audience: &'a str,
    pub issuer: Option<&'a str>,
    pub in_response_to: Option<&'a str>,
    pub allow_sha1: bool,
    pub now: DateTime<Utc>,
}

/// Parses and verifies a SAML `Response`: status, a valid signature over the Response or the
/// Assertion by one of `certs`, destination / audience / time window / InResponseTo.
pub fn verify_response(
    xml: &str,
    certs: &[RsaPublicKey],
    o: VerifyOptions<'_>,
) -> Result<Assertion> {
    let doc = Document::parse(xml)
        .map_err(|e| Error::unauthenticated(format!("saml: malformed xml: {e}")))?;
    let root = doc.root_element();
    if !(root.tag_name().name() == "Response" && root.tag_name().namespace() == Some(NS_SAMLP)) {
        return Err(Error::unauthenticated("saml: not a samlp:Response"));
    }
    if certs.is_empty() {
        return Err(Error::unavailable("saml: no idp certificates configured"));
    }
    let status = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Status")
        .and_then(|s| {
            s.children()
                .find(|n| n.is_element() && n.tag_name().name() == "StatusCode")
        })
        .and_then(|c| c.attribute("Value"))
        .unwrap_or("");
    if status != STATUS_SUCCESS {
        return Err(Error::unauthenticated(format!(
            "saml: idp returned status {status:?}"
        )));
    }
    if let Some(dest) = root.attribute("Destination") {
        if dest != o.acs_url {
            return Err(Error::unauthenticated(
                "saml: response destination mismatch",
            ));
        }
    }
    if let (Some(expected), Some(irt)) = (o.in_response_to, root.attribute("InResponseTo")) {
        if irt != expected {
            return Err(Error::unauthenticated(
                "saml: response InResponseTo mismatch",
            ));
        }
    }
    if root
        .children()
        .any(|n| n.is_element() && n.tag_name().name() == "EncryptedAssertion")
    {
        return Err(Error::unauthenticated(
            "saml: encrypted assertions are not supported; configure the idp to sign, not encrypt",
        ));
    }
    let assertion = root
        .children()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "Assertion"
                && n.tag_name().namespace() == Some(NS_SAML)
        })
        .ok_or_else(|| Error::unauthenticated("saml: response has no assertion"))?;

    // Signature: on the Response (covering everything) or on the Assertion.
    let response_signed = signature_of(root)
        .map(|sig| verify_signature(root, sig, certs, o.allow_sha1))
        .transpose()?
        .is_some();
    let assertion_signed = signature_of(assertion)
        .map(|sig| verify_signature(assertion, sig, certs, o.allow_sha1))
        .transpose()?
        .is_some();
    if !response_signed && !assertion_signed {
        return Err(Error::unauthenticated(
            "saml: neither response nor assertion is signed",
        ));
    }

    let issuer = child_text(assertion, "Issuer").unwrap_or_default();
    if let Some(expected) = o.issuer {
        if issuer != expected {
            return Err(Error::unauthenticated("saml: unexpected assertion issuer"));
        }
    }
    if let Some(cond) = assertion
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Conditions")
    {
        if let Some(nb) = cond.attribute("NotBefore").and_then(parse_time) {
            if o.now + chrono::Duration::seconds(CLOCK_SKEW_SECS) < nb {
                return Err(Error::unauthenticated("saml: assertion not yet valid"));
            }
        }
        if let Some(na) = cond.attribute("NotOnOrAfter").and_then(parse_time) {
            if o.now - chrono::Duration::seconds(CLOCK_SKEW_SECS) >= na {
                return Err(Error::unauthenticated("saml: assertion expired"));
            }
        }
        let audiences: Vec<&str> = cond
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Audience")
            .filter_map(|n| n.text())
            .map(str::trim)
            .collect();
        if !audiences.is_empty() && !audiences.contains(&o.audience) {
            return Err(Error::unauthenticated("saml: assertion audience mismatch"));
        }
    }
    let subject = assertion
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Subject");
    let name_id = subject
        .and_then(|s| child_text(s, "NameID"))
        .unwrap_or_default();
    let mut in_response_to = None;
    if let Some(scd) = subject.and_then(|s| {
        s.descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "SubjectConfirmationData")
    }) {
        if let Some(r) = scd.attribute("Recipient") {
            if r != o.acs_url {
                return Err(Error::unauthenticated(
                    "saml: subject confirmation recipient mismatch",
                ));
            }
        }
        if let Some(na) = scd.attribute("NotOnOrAfter").and_then(parse_time) {
            if o.now - chrono::Duration::seconds(CLOCK_SKEW_SECS) >= na {
                return Err(Error::unauthenticated("saml: subject confirmation expired"));
            }
        }
        if let Some(irt) = scd.attribute("InResponseTo") {
            if let Some(expected) = o.in_response_to {
                if irt != expected {
                    return Err(Error::unauthenticated(
                        "saml: assertion InResponseTo mismatch",
                    ));
                }
            }
            in_response_to = Some(irt.to_string());
        }
    }
    if o.in_response_to.is_some()
        && root.attribute("InResponseTo").is_none()
        && in_response_to.is_none()
    {
        return Err(Error::unauthenticated(
            "saml: unsolicited response rejected",
        ));
    }
    let mut attributes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for st in assertion
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "AttributeStatement")
    {
        for a in st
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Attribute")
        {
            let Some(name) = a.attribute("Name") else {
                continue;
            };
            let values: Vec<String> = a
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "AttributeValue")
                .map(|v| v.text().unwrap_or("").trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            attributes
                .entry(name.to_string())
                .or_default()
                .extend(values);
        }
    }
    let session_index = assertion
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "AuthnStatement")
        .and_then(|n| n.attribute("SessionIndex"))
        .map(str::to_string);
    Ok(Assertion {
        name_id: name_id.trim().to_string(),
        attributes,
        issuer: issuer.trim().to_string(),
        in_response_to,
        session_index,
    })
}

fn signature_of<'a>(el: Node<'a, 'a>) -> Option<Node<'a, 'a>> {
    el.children().find(|n| {
        n.is_element()
            && n.tag_name().name() == "Signature"
            && n.tag_name().namespace() == Some(NS_DS)
    })
}

fn child_text<'a>(el: Node<'a, 'a>, name: &str) -> Option<String> {
    el.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Verifies `sig` (a `ds:Signature` child of `signed`) against `signed`: the single Reference
/// must point at `signed` (`URI="#ID"` or `""`), the digest of the enveloped-canonicalized element
/// must match and the RSA signature over canonical `SignedInfo` must verify with one of `certs`.
fn verify_signature(
    signed: Node<'_, '_>,
    sig: Node<'_, '_>,
    certs: &[RsaPublicKey],
    allow_sha1: bool,
) -> Result<()> {
    let signed_info = sig
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "SignedInfo")
        .ok_or_else(|| Error::unauthenticated("saml: signature without SignedInfo"))?;
    let sig_alg = signed_info
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "SignatureMethod")
        .and_then(|n| n.attribute("Algorithm"))
        .unwrap_or("");
    let references: Vec<Node> = signed_info
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Reference")
        .collect();
    if references.len() != 1 {
        return Err(Error::unauthenticated(
            "saml: signature must have exactly one reference",
        ));
    }
    let reference = references[0];
    let uri = reference.attribute("URI").unwrap_or("");
    let id = signed.attribute("ID").unwrap_or("");
    if !(uri.is_empty() || (uri.starts_with('#') && !id.is_empty() && &uri[1..] == id)) {
        return Err(Error::unauthenticated(
            "saml: signature reference does not cover the signed element",
        ));
    }
    let mut enveloped = false;
    let mut inclusive: Vec<String> = Vec::new();
    for t in reference
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Transform")
    {
        match t.attribute("Algorithm").unwrap_or("") {
            TRANSFORM_ENVELOPED => enveloped = true,
            "http://www.w3.org/2001/10/xml-exc-c14n#"
            | "http://www.w3.org/2001/10/xml-exc-c14n#WithComments" => {
                if let Some(pl) = t
                    .children()
                    .find(|n| n.is_element() && n.tag_name().name() == "InclusiveNamespaces")
                    .and_then(|n| n.attribute("PrefixList"))
                {
                    inclusive.extend(pl.split_whitespace().map(str::to_string));
                }
            }
            "http://www.w3.org/TR/2001/REC-xml-c14n-20010315"
            | "http://www.w3.org/TR/2001/REC-xml-c14n-20010315#WithComments" => {
                inclusive.push("*".into());
            }
            other => {
                return Err(Error::unauthenticated(format!(
                    "saml: unsupported transform {other}"
                )))
            }
        }
    }
    if !enveloped {
        return Err(Error::unauthenticated(
            "saml: reference lacks the enveloped-signature transform",
        ));
    }
    let digest_alg = reference
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "DigestMethod")
        .and_then(|n| n.attribute("Algorithm"))
        .unwrap_or("");
    let digest_value = reference
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "DigestValue")
        .and_then(|n| n.text())
        .map(|t| t.split_whitespace().collect::<String>())
        .unwrap_or_default();
    let expected = STANDARD
        .decode(&digest_value)
        .map_err(|_| Error::unauthenticated("saml: DigestValue is not base64"))?;
    let canon = canonicalize(signed, Some(sig), &inclusive);
    let actual = match digest_alg {
        DIGEST_SHA256 => Sha256::digest(&canon).to_vec(),
        DIGEST_SHA1 if allow_sha1 => Sha1::digest(&canon).to_vec(),
        other => {
            return Err(Error::unauthenticated(format!(
                "saml: unsupported digest {other}"
            )))
        }
    };
    if actual != expected {
        return Err(Error::unauthenticated(
            "saml: digest mismatch — the signed content was altered",
        ));
    }
    let si_inclusive: Vec<String> = signed_info
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "CanonicalizationMethod")
        .and_then(|c| {
            c.children()
                .find(|n| n.is_element() && n.tag_name().name() == "InclusiveNamespaces")
                .and_then(|n| n.attribute("PrefixList"))
        })
        .map(|pl| pl.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let si_canon = canonicalize(signed_info, None, &si_inclusive);
    let sig_value = sig
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "SignatureValue")
        .and_then(|n| n.text())
        .map(|t| t.split_whitespace().collect::<String>())
        .unwrap_or_default();
    let sig_bytes = STANDARD
        .decode(&sig_value)
        .map_err(|_| Error::unauthenticated("saml: SignatureValue is not base64"))?;
    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|_| Error::unauthenticated("saml: malformed signature value"))?;
    let ok = certs.iter().any(|k| match sig_alg {
        ALG_RSA_SHA256 => VerifyingKey::<Sha256>::new(k.clone())
            .verify(&si_canon, &signature)
            .is_ok(),
        ALG_RSA_SHA1 if allow_sha1 => VerifyingKey::<Sha1>::new(k.clone())
            .verify(&si_canon, &signature)
            .is_ok(),
        _ => false,
    });
    if !ok {
        if sig_alg != ALG_RSA_SHA256 && !(allow_sha1 && sig_alg == ALG_RSA_SHA1) {
            return Err(Error::unauthenticated(format!(
                "saml: unsupported signature algorithm {sig_alg}"
            )));
        }
        return Err(Error::unauthenticated(
            "saml: signature does not verify with any configured idp certificate",
        ));
    }
    Ok(())
}

/// Exclusive XML canonicalization (xml-exc-c14n, no comments) of `el`, omitting the subtree
/// `omit` (enveloped signature). Prefixes in `inclusive` (or `*`) are rendered inclusively.
pub fn canonicalize(el: Node<'_, '_>, omit: Option<Node<'_, '_>>, inclusive: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    // namespaces in scope at the apex that have to be re-declared when used
    let mut rendered: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    write_node(el, omit, inclusive, &mut rendered, &mut out);
    out
}

fn write_node(
    n: Node<'_, '_>,
    omit: Option<Node<'_, '_>>,
    inclusive: &[String],
    rendered: &mut Vec<BTreeMap<String, String>>,
    out: &mut Vec<u8>,
) {
    if omit.is_some_and(|o| o == n) {
        return;
    }
    match n.node_type() {
        roxmltree::NodeType::Element => {
            let tag = n.tag_name();
            let prefix = n.lookup_prefix(tag.namespace().unwrap_or("")).unwrap_or("");
            let qname = if prefix.is_empty() {
                tag.name().to_string()
            } else {
                format!("{prefix}:{}", tag.name())
            };
            out.extend_from_slice(b"<");
            out.extend_from_slice(qname.as_bytes());

            // visibly utilised namespaces: element's own + attributes' prefixes (+ inclusive list)
            let mut used: BTreeMap<String, String> = BTreeMap::new();
            let all_inclusive = inclusive.iter().any(|p| p == "*");
            let ns_uri = |p: &str| -> Option<String> {
                n.namespaces()
                    .find(|ns| ns.name().unwrap_or("") == p)
                    .map(|ns| ns.uri().to_string())
            };
            if let Some(uri) = ns_uri(prefix) {
                used.insert(prefix.to_string(), uri);
            } else if prefix.is_empty() {
                used.insert(String::new(), String::new());
            }
            for a in n.attributes() {
                if let Some(ns) = a.namespace() {
                    if let Some(p) = n.lookup_prefix(ns) {
                        if !p.is_empty() {
                            used.insert(p.to_string(), ns.to_string());
                        }
                    }
                }
            }
            for ns in n.namespaces() {
                let p = ns.name().unwrap_or("");
                if all_inclusive
                    || inclusive
                        .iter()
                        .any(|i| (i == "#default" && p.is_empty()) || i == p)
                {
                    used.insert(p.to_string(), ns.uri().to_string());
                }
            }
            let parent_scope = rendered.last().cloned().unwrap_or_default();
            let mut scope = parent_scope.clone();
            let mut decls: Vec<(String, String)> = Vec::new();
            for (p, uri) in &used {
                let already = parent_scope
                    .get(p)
                    .map(|u| u == uri)
                    .unwrap_or(p.is_empty() && uri.is_empty());
                if !already {
                    decls.push((p.clone(), uri.clone()));
                    scope.insert(p.clone(), uri.clone());
                }
            }
            decls.sort();
            for (p, uri) in &decls {
                if p.is_empty() {
                    if uri.is_empty() {
                        continue;
                    }
                    out.extend_from_slice(b" xmlns=\"");
                } else {
                    out.extend_from_slice(b" xmlns:");
                    out.extend_from_slice(p.as_bytes());
                    out.extend_from_slice(b"=\"");
                }
                out.extend_from_slice(escape_attr(uri).as_bytes());
                out.extend_from_slice(b"\"");
            }
            let mut attrs: Vec<(String, String, String)> = n
                .attributes()
                .map(|a| {
                    let ns = a.namespace().unwrap_or("").to_string();
                    let p = a
                        .namespace()
                        .and_then(|ns| n.lookup_prefix(ns))
                        .unwrap_or("");
                    let q = if p.is_empty() {
                        a.name().to_string()
                    } else {
                        format!("{p}:{}", a.name())
                    };
                    (
                        ns,
                        a.name().to_string(),
                        format!(" {q}=\"{}\"", escape_attr(a.value())),
                    )
                })
                .collect();
            attrs.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
            for (_, _, s) in &attrs {
                out.extend_from_slice(s.as_bytes());
            }
            out.extend_from_slice(b">");
            rendered.push(scope);
            for c in n.children() {
                write_node(c, omit, inclusive, rendered, out);
            }
            rendered.pop();
            out.extend_from_slice(b"</");
            out.extend_from_slice(qname.as_bytes());
            out.extend_from_slice(b">");
        }
        roxmltree::NodeType::Text => {
            out.extend_from_slice(escape_text(n.text().unwrap_or("")).as_bytes());
        }
        _ => {}
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#xD;")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
        .replace('\t', "&#x9;")
        .replace('\n', "&#xA;")
        .replace('\r', "&#xD;")
}

fn xml_escape(s: &str) -> String {
    escape_attr(s)
}

/// Accepts PEM (`-----BEGIN CERTIFICATE-----`) or bare base64 DER as found in IdP metadata.
pub fn parse_cert(input: &str) -> Result<RsaPublicKey> {
    let b64: String = input
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("")
        .split_whitespace()
        .collect();
    let der = STANDARD
        .decode(b64)
        .map_err(|_| Error::invalid("certificate is not base64"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&der)
        .map_err(|e| Error::invalid(format!("certificate: {e}")))?;
    RsaPublicKey::from_pkcs1_der(&cert.tbs_certificate.subject_pki.subject_public_key.data)
        .map_err(|e| Error::invalid(format!("certificate public key is not RSA: {e}")))
}

fn random_token() -> String {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{
        pkcs1v15::SigningKey,
        signature::{SignatureEncoding, Signer},
        RsaPrivateKey,
    };

    fn canon_str(xml: &str) -> String {
        let d = Document::parse(xml).unwrap();
        String::from_utf8(canonicalize(d.root_element(), None, &[])).unwrap()
    }

    #[test]
    fn exclusive_c14n_basics() {
        // attribute order, namespace declaration only where visibly used, unused xmlns dropped
        let c = canon_str(
            r#"<a:root xmlns:a="urn:a" xmlns:unused="urn:u" z="1" b="2"><a:child xmlns:a="urn:a" c="x &amp; y"/><plain>t&lt;</plain></a:root>"#,
        );
        assert_eq!(
            c,
            r#"<a:root xmlns:a="urn:a" b="2" z="1"><a:child c="x &amp; y"></a:child><plain>t&lt;</plain></a:root>"#
        );
        // default namespace redeclared on the apex only when used, attributes ordered by (ns uri, name)
        let c = canon_str(r#"<r xmlns="urn:d" xmlns:x="urn:x" x:b="1" a="2"><y/></r>"#);
        assert_eq!(
            c,
            r#"<r xmlns="urn:d" xmlns:x="urn:x" a="2" x:b="1"><y></y></r>"#
        );
    }

    fn signed_response(key: &RsaPrivateKey, assertion_body: &str, tamper: bool) -> String {
        let assertion = format!(
            r#"<saml:Assertion xmlns:saml="{NS_SAML}" ID="_a1" Version="2.0" IssueInstant="2026-01-01T00:00:00Z"><saml:Issuer>https://idp.example.com</saml:Issuer>{{SIG}}{assertion_body}</saml:Assertion>"#
        );
        let unsigned = assertion.replace("{SIG}", "");
        let doc = Document::parse(&unsigned).unwrap();
        let digest = Sha256::digest(canonicalize(doc.root_element(), None, &[]));
        let signed_info = format!(
            r##"<ds:SignedInfo xmlns:ds="{NS_DS}"><ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></ds:CanonicalizationMethod><ds:SignatureMethod Algorithm="{ALG_RSA_SHA256}"></ds:SignatureMethod><ds:Reference URI="#_a1"><ds:Transforms><ds:Transform Algorithm="{TRANSFORM_ENVELOPED}"></ds:Transform><ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></ds:Transform></ds:Transforms><ds:DigestMethod Algorithm="{DIGEST_SHA256}"></ds:DigestMethod><ds:DigestValue>{}</ds:DigestValue></ds:Reference></ds:SignedInfo>"##,
            STANDARD.encode(digest)
        );
        let si_doc = Document::parse(&signed_info).unwrap();
        let si_canon = canonicalize(si_doc.root_element(), None, &[]);
        let sig = SigningKey::<Sha256>::new(key.clone()).sign(&si_canon);
        let signature = format!(
            r#"<ds:Signature xmlns:ds="{NS_DS}">{signed_info}<ds:SignatureValue>{}</ds:SignatureValue></ds:Signature>"#,
            STANDARD.encode(sig.to_bytes())
        );
        let mut signed = assertion.replace("{SIG}", &signature);
        if tamper {
            signed = signed.replace("alice@example.com", "mallory@example.com");
        }
        format!(
            r#"<samlp:Response xmlns:samlp="{NS_SAMLP}" xmlns:saml="{NS_SAML}" ID="_r1" Version="2.0" IssueInstant="2026-01-01T00:00:00Z" Destination="https://z.example.com/console/auth/saml/acs" InResponseTo="_req1"><saml:Issuer>https://idp.example.com</saml:Issuer><samlp:Status><samlp:StatusCode Value="{STATUS_SUCCESS}"></samlp:StatusCode></samlp:Status>{signed}</samlp:Response>"#
        )
    }

    fn body() -> String {
        r#"<saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">alice@example.com</saml:NameID><saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData InResponseTo="_req1" NotOnOrAfter="2099-01-01T00:00:00Z" Recipient="https://z.example.com/console/auth/saml/acs"></saml:SubjectConfirmationData></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="2020-01-01T00:00:00Z" NotOnOrAfter="2099-01-01T00:00:00Z"><saml:AudienceRestriction><saml:Audience>https://z.example.com/console/auth/saml/metadata</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AuthnStatement AuthnInstant="2026-01-01T00:00:00Z" SessionIndex="s1"></saml:AuthnStatement><saml:AttributeStatement><saml:Attribute Name="groups"><saml:AttributeValue>admins</saml:AttributeValue><saml:AttributeValue>devs</saml:AttributeValue></saml:Attribute><saml:Attribute Name="name"><saml:AttributeValue>Alice</saml:AttributeValue></saml:Attribute></saml:AttributeStatement>"#.to_string()
    }

    fn opts<'a>(now: DateTime<Utc>) -> VerifyOptions<'a> {
        VerifyOptions {
            acs_url: "https://z.example.com/console/auth/saml/acs",
            audience: "https://z.example.com/console/auth/saml/metadata",
            issuer: Some("https://idp.example.com"),
            in_response_to: Some("_req1"),
            allow_sha1: false,
            now,
        }
    }

    #[test]
    fn verifies_signed_assertion_and_rejects_tampering() {
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pk = key.to_public_key();
        let other = RsaPrivateKey::new(&mut rng, 2048).unwrap().to_public_key();
        let now = Utc::now();

        let a = verify_response(
            &signed_response(&key, &body(), false),
            std::slice::from_ref(&pk),
            opts(now),
        )
        .expect("valid");
        assert_eq!(a.name_id, "alice@example.com");
        assert_eq!(a.attributes["groups"], vec!["admins", "devs"]);
        assert_eq!(a.session_index.as_deref(), Some("s1"));

        let err = verify_response(
            &signed_response(&key, &body(), true),
            std::slice::from_ref(&pk),
            opts(now),
        )
        .unwrap_err();
        assert!(err.to_string().contains("digest"), "{err}");
        assert!(
            verify_response(&signed_response(&key, &body(), false), &[other], opts(now)).is_err()
        );
        let mut wrong = opts(now);
        wrong.in_response_to = Some("_other");
        assert!(verify_response(
            &signed_response(&key, &body(), false),
            std::slice::from_ref(&pk),
            wrong
        )
        .is_err());
        let mut aud = opts(now);
        aud.audience = "https://elsewhere";
        assert!(verify_response(
            &signed_response(&key, &body(), false),
            std::slice::from_ref(&pk),
            aud
        )
        .is_err());
        let expired = body().replace(
            r#"NotOnOrAfter="2099-01-01T00:00:00Z"><saml:AudienceRestriction"#,
            r#"NotOnOrAfter="2021-01-01T00:00:00Z"><saml:AudienceRestriction"#,
        );
        assert!(verify_response(
            &signed_response(&key, &expired, false),
            std::slice::from_ref(&pk),
            opts(now)
        )
        .is_err());
        let unsigned = signed_response(&key, &body(), false);
        let stripped = {
            let start = unsigned.find("<ds:Signature").unwrap();
            let end = unsigned.find("</ds:Signature>").unwrap() + "</ds:Signature>".len();
            format!("{}{}", &unsigned[..start], &unsigned[end..])
        };
        assert!(verify_response(&stripped, &[pk], opts(now)).is_err());
    }

    #[test]
    fn authn_request_roundtrip() {
        let svc = SamlService::new(
            Some(SamlConfig {
                idp_sso_url: "https://idp.example.com/sso".into(),
                idp_entity_id: String::new(),
                idp_certs: vec![],
                sp_entity_id: String::new(),
                acs_url: "https://z.example.com/console/auth/saml/acs".into(),
                email_attribute: "email".into(),
                name_attribute: "name".into(),
                role_attribute: "groups".into(),
                role_map: Default::default(),
                default_role: None,
                allowed_domains: vec![],
                allow_sha1: false,
            }),
            "secret",
        );
        let start = svc.begin_login().unwrap();
        let url = url::Url::parse(&start.redirect_url).unwrap();
        let req = url
            .query_pairs()
            .find(|(k, _)| k == "SAMLRequest")
            .unwrap()
            .1
            .to_string();
        let raw = STANDARD.decode(req).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&raw[..]), &mut xml)
            .unwrap();
        assert!(xml.contains(
            "AssertionConsumerServiceURL=\"https://z.example.com/console/auth/saml/acs\""
        ));
        assert!(xml.contains(
            "<saml:Issuer>https://z.example.com/console/auth/saml/metadata</saml:Issuer>"
        ));
        assert!(svc
            .metadata()
            .unwrap()
            .contains("entityID=\"https://z.example.com/console/auth/saml/metadata\""));
    }
}
