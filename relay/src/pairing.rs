//! Joining new devices.
//!
//! While no device is registered, the relay keeps a one-time *first code*
//! and prints it to its log; the first device joins with it. After that only
//! connected devices can invite: the inviting device shows a code like
//! `37-4821-1937`, where `37` names the pairing on the relay and the rest is
//! a password the relay never sees. Both devices run SPAKE2 over the relay
//! with that password, the joiner proves it derived the same key, and only
//! then does the inviter approve, which registers the joiner and hands it the
//! sharing key sealed under the SPAKE2 key. A wrong guess ends the pairing,
//! so a guessing attacker gets one try per code. The joiner's name reaches
//! the inviter sealed under the SPAKE2 key; the inviter re-seals it with the
//! sharing key as the label the relay stores.

use crate::store::random_hex;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PAIRING_TTL: Duration = Duration::from_secs(600);
pub const FIRST_CODE_TTL: Duration = Duration::from_secs(1800);
pub const MAX_FIRST_CODE_FAILURES: u32 = 5;

struct FirstCode {
    code: String,
    issued: Instant,
    failures: u32,
}

#[derive(Clone)]
pub struct Answer {
    pub msg_b: Vec<u8>,
    /// Sealed by the joiner under the SPAKE2 key, proving it knew the code.
    pub confirm: Vec<u8>,
    /// The joiner's name, sealed under the SPAKE2 key.
    pub name: Vec<u8>,
}

#[derive(Clone)]
pub struct Approved {
    pub device_id: String,
    pub token: String,
    /// The sharing key, sealed under the SPAKE2 key.
    pub payload: Vec<u8>,
}

struct Pairing {
    owner: String,
    msg_a: Vec<u8>,
    created: Instant,
    ticket: Option<String>,
    answer: Option<Answer>,
    approved: Option<Approved>,
}

#[derive(Debug, PartialEq)]
pub enum PairingError {
    /// No such pairing, or it expired or was cancelled.
    Gone,
    /// Someone already joined this pairing.
    Taken,
    /// Wrong ticket, wrong owner or wrong step.
    Refused,
}

#[derive(Default)]
pub struct Joining {
    first: Mutex<Option<FirstCode>>,
    pairings: Mutex<HashMap<String, Pairing>>,
}

/// `n` random decimal digits.
fn digits(n: usize) -> String {
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut byte = [0u8; 1];
        getrandom::fill(&mut byte).expect("system randomness");
        if byte[0] < 250 {
            out.push(char::from(b'0' + byte[0] % 10));
        }
    }
    out
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

impl Joining {
    /// Keeps a valid first code while no device exists. Returns a code when a
    /// new one was issued so the caller can print it.
    pub fn refresh_first_code(&self, devices: i64) -> Option<String> {
        let mut first = self.first.lock().unwrap();
        if devices > 0 {
            *first = None;
            return None;
        }
        let valid = first
            .as_ref()
            .is_some_and(|f| f.issued.elapsed() < FIRST_CODE_TTL && f.failures < MAX_FIRST_CODE_FAILURES);
        if valid {
            return None;
        }
        let raw = digits(12);
        let code = format!("{}-{}-{}", &raw[..4], &raw[4..8], &raw[8..]);
        *first = Some(FirstCode { code: raw, issued: Instant::now(), failures: 0 });
        Some(code)
    }

    /// Consumes the first code if `code` matches it.
    pub fn take_first_code(&self, code: &str) -> bool {
        let mut first = self.first.lock().unwrap();
        let Some(current) = first.as_mut() else { return false };
        if current.issued.elapsed() >= FIRST_CODE_TTL || current.failures >= MAX_FIRST_CODE_FAILURES {
            return false;
        }
        if same(&normalize_code(code), &current.code) {
            *first = None;
            true
        } else {
            current.failures += 1;
            false
        }
    }

    fn sweep(pairings: &mut HashMap<String, Pairing>) {
        pairings.retain(|_, p| p.created.elapsed() < PAIRING_TTL);
    }

    /// Opens a pairing for `owner`, replacing any earlier one it had.
    /// Returns the nameplate.
    pub fn create(&self, owner: &str, msg_a: Vec<u8>) -> String {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        pairings.retain(|_, p| p.owner != owner);
        // Two digits normally; never four, which would look like a first code.
        let width = if pairings.len() < 60 { 2 } else { 3 };
        let nameplate = loop {
            let candidate = digits(width);
            if !candidate.starts_with('0') && !pairings.contains_key(&candidate) {
                break candidate;
            }
        };
        pairings.insert(
            nameplate.clone(),
            Pairing { owner: owner.into(), msg_a, created: Instant::now(), ticket: None, answer: None, approved: None },
        );
        nameplate
    }

    /// The joiner claims a pairing. Only the first claim succeeds.
    pub fn join(&self, nameplate: &str) -> Result<(Vec<u8>, String), PairingError> {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        let pairing = pairings.get_mut(nameplate).ok_or(PairingError::Gone)?;
        if pairing.ticket.is_some() {
            return Err(PairingError::Taken);
        }
        let ticket = random_hex(16);
        pairing.ticket = Some(ticket.clone());
        Ok((pairing.msg_a.clone(), ticket))
    }

    pub fn answer(&self, nameplate: &str, ticket: &str, answer: Answer) -> Result<(), PairingError> {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        let pairing = pairings.get_mut(nameplate).ok_or(PairingError::Gone)?;
        if !pairing.ticket.as_deref().is_some_and(|t| same(t, ticket)) || pairing.answer.is_some() {
            return Err(PairingError::Refused);
        }
        pairing.answer = Some(answer);
        Ok(())
    }

    /// What the owner sees: the joiner's answer once it arrives.
    pub fn answer_for(&self, nameplate: &str, owner: &str) -> Result<Option<Answer>, PairingError> {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        let pairing = pairings.get(nameplate).ok_or(PairingError::Gone)?;
        if pairing.owner != owner {
            return Err(PairingError::Gone);
        }
        Ok(pairing.answer.clone())
    }

    /// Records the owner's approval once the joiner has answered. `register`
    /// creates the device and returns its id and token.
    pub fn approve(
        &self,
        nameplate: &str,
        owner: &str,
        payload: Vec<u8>,
        register: impl FnOnce() -> Result<(String, String), String>,
    ) -> Result<Result<String, String>, PairingError> {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        let pairing = pairings.get_mut(nameplate).ok_or(PairingError::Gone)?;
        if pairing.owner != owner || pairing.approved.is_some() {
            return Err(PairingError::Refused);
        }
        if pairing.answer.is_none() {
            return Err(PairingError::Refused);
        }
        Ok(register().map(|(device_id, token)| {
            pairing.approved = Some(Approved { device_id: device_id.clone(), token, payload });
            device_id
        }))
    }

    /// The joiner's result: `None` while waiting; the pairing ends once read.
    pub fn result(&self, nameplate: &str, ticket: &str) -> Result<Option<Approved>, PairingError> {
        let mut pairings = self.pairings.lock().unwrap();
        Self::sweep(&mut pairings);
        let pairing = pairings.get(nameplate).ok_or(PairingError::Gone)?;
        if !pairing.ticket.as_deref().is_some_and(|t| same(t, ticket)) {
            return Err(PairingError::Refused);
        }
        let approved = pairing.approved.clone();
        if approved.is_some() {
            pairings.remove(nameplate);
        }
        Ok(approved)
    }

    pub fn cancel(&self, nameplate: &str, owner: &str) {
        let mut pairings = self.pairings.lock().unwrap();
        if pairings.get(nameplate).is_some_and(|p| p.owner == owner) {
            pairings.remove(nameplate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Answer {
        Answer { msg_b: b"b".to_vec(), confirm: b"c".to_vec(), name: b"name".to_vec() }
    }

    #[test]
    fn first_code_exists_only_without_devices_and_is_single_use() {
        let joining = Joining::default();
        assert_eq!(joining.refresh_first_code(1), None);
        let code = joining.refresh_first_code(0).unwrap();
        assert_eq!(code.len(), 14);
        assert_eq!(joining.refresh_first_code(0), None, "still valid, not reissued");
        assert!(joining.take_first_code(&code.replace('-', " ")), "separators are ignored");
        assert!(!joining.take_first_code(&code), "used up");
        assert!(joining.refresh_first_code(0).is_some(), "a new one once used");
        assert_eq!(joining.refresh_first_code(1), None);
        assert!(!joining.take_first_code(&code), "cleared when a device exists");
    }

    #[test]
    fn first_code_is_replaced_after_too_many_wrong_guesses() {
        let joining = Joining::default();
        let code = joining.refresh_first_code(0).unwrap();
        for _ in 0..MAX_FIRST_CODE_FAILURES {
            assert!(!joining.take_first_code("0000-0000-0000"));
        }
        assert!(!joining.take_first_code(&code), "locked after the limit");
        let next = joining.refresh_first_code(0).unwrap();
        assert!(joining.take_first_code(&next));
    }

    #[test]
    fn pairing_runs_once_through_its_steps() {
        let joining = Joining::default();
        let np = joining.create("owner", b"a".to_vec());
        assert!(np.len() == 2 && !np.starts_with('0'));
        assert_eq!(joining.answer_for(&np, "owner").unwrap().map(|a| a.msg_b), None);
        let (msg_a, ticket) = joining.join(&np).unwrap();
        assert_eq!(msg_a, b"a");
        assert_eq!(joining.join(&np).unwrap_err(), PairingError::Taken, "one joiner per code");
        assert_eq!(joining.answer(&np, "wrong", answer()), Err(PairingError::Refused));
        joining.answer(&np, &ticket, answer()).unwrap();
        assert_eq!(joining.answer(&np, &ticket, answer()), Err(PairingError::Refused), "answers once");
        assert_eq!(joining.result(&np, &ticket).unwrap().map(|a| a.token), None, "not approved yet");
        assert_eq!(joining.answer_for(&np, "intruder").err(), Some(PairingError::Gone));
        assert!(joining.approve(&np, "intruder", vec![], || unreachable!()).is_err());
        let id = joining
            .approve(&np, "owner", b"sealed key".to_vec(), || Ok(("dev".into(), "pr1_t".into())))
            .unwrap()
            .unwrap();
        assert_eq!(id, "dev");
        let approved = joining.result(&np, &ticket).unwrap().unwrap();
        assert_eq!((approved.token.as_str(), approved.payload.as_slice()), ("pr1_t", &b"sealed key"[..]));
        assert_eq!(joining.result(&np, &ticket).err(), Some(PairingError::Gone), "read once");
    }

    #[test]
    fn new_pairing_replaces_the_owners_previous_one_and_cancel_ends_it() {
        let joining = Joining::default();
        let first = joining.create("owner", vec![]);
        let second = joining.create("owner", vec![]);
        assert_eq!(joining.join(&first).err(), if first == second { None } else { Some(PairingError::Gone) });
        joining.cancel(&second, "someone else");
        assert!(joining.answer_for(&second, "owner").is_ok());
        joining.cancel(&second, "owner");
        assert_eq!(joining.join(&second).err(), Some(PairingError::Gone));
    }
}
