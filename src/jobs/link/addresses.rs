use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use kebbie_hub_api::{AnnouncedAddresses, MessageUpload};
use kebbie_hub_protocol::{AccountKey, Addresses, Id, LinkAddress, Network, Payload};

use super::super::cluster::{self, Cluster};
use crate::hub::http::{Client, HubError};
use crate::hub::identity::Identity;
use crate::sh::Result;

fn tailscale(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64,
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

// The ranges no public interface carries: RFC 1918, carrier-grade NAT and IPv6 unique-local.
fn private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn tailscale_interface(name: &str) -> bool {
    name.starts_with("tailscale") || name.starts_with("utun")
}

fn usable(ip: &IpAddr) -> bool {
    let link_local = matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
    !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() && !link_local
}

// The private-network addresses among this machine's interfaces: Tailscale's as tailscale ip says or by their ranges on a tailscale or utun interface, WireGuard's by interface and only in a private range; tailnet first.
pub fn classify(interfaces: &[(String, IpAddr)], wireguard: &BTreeSet<String>, tailnet: &[IpAddr]) -> Vec<LinkAddress> {
    let mut found: Vec<(Network, IpAddr)> = Vec::new();
    let on_tailscale = interfaces.iter().filter(|(name, ip)| tailscale_interface(name) && tailscale(ip)).map(|(_, ip)| ip);
    for ip in tailnet.iter().filter(|ip| private(ip)).chain(on_tailscale).filter(|ip| usable(ip)) {
        found.push((Network::Tailscale, *ip));
    }
    for (_, ip) in interfaces.iter().filter(|(name, ip)| usable(ip) && private(ip) && (wireguard.contains(name) || name.starts_with("wg"))) {
        found.push((Network::Wireguard, *ip));
    }
    let mut seen = BTreeSet::new();
    found.into_iter().filter(|(_, ip)| seen.insert(*ip)).map(|(network, ip)| LinkAddress { network, ip: ip.to_string() }).take(kebbie_hub_protocol::MAX_LINK_ADDRESSES).collect()
}

fn interfaces() -> Vec<(String, IpAddr)> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills list, which is read here and released with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return found;
    }
    let mut at = list;
    while !at.is_null() {
        // SAFETY: at is a node of the list getifaddrs returned, alive until freeifaddrs.
        let entry = unsafe { &*at };
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_name is a NUL-terminated name and ifa_addr a sockaddr of the family it names.
            let (name, family) = unsafe { (CStr::from_ptr(entry.ifa_name).to_string_lossy().to_string(), i32::from((*entry.ifa_addr).sa_family)) };
            let ip = match family {
                // SAFETY: an AF_INET address is a sockaddr_in.
                libc::AF_INET => Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr })))),
                // SAFETY: an AF_INET6 address is a sockaddr_in6.
                libc::AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from(unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr }))),
                _ => None,
            };
            found.extend(ip.map(|ip| (name, ip)));
        }
        at = entry.ifa_next;
    }
    // SAFETY: list came from getifaddrs and is released once.
    unsafe { libc::freeifaddrs(list) };
    found
}

fn quick(program: &str, args: &[&str]) -> String {
    let until = Instant::now() + Duration::from_secs(2);
    let mut command = std::process::Command::new(program);
    command.args(args);
    match crate::sh::output_until(command, &|| Instant::now() >= until) {
        Ok(Some(done)) if done.code == Some(0) => done.stdout,
        _ => String::new(),
    }
}

// WireGuard's interfaces by wg show, which needs root on most machines, and by the kernel's own word for one on Linux.
fn wireguard_names(interfaces: &[(String, IpAddr)]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = quick("wg", &["show", "interfaces"]).split_whitespace().map(str::to_string).collect();
    for (name, _) in interfaces {
        if std::fs::read_to_string(format!("/sys/class/net/{name}/uevent")).is_ok_and(|text| text.lines().any(|line| line == "DEVTYPE=wireguard")) {
            names.insert(name.clone());
        }
    }
    names
}

pub fn local() -> Vec<LinkAddress> {
    let interfaces = interfaces();
    let tailnet: Vec<IpAddr> = quick("tailscale", &["ip"]).lines().filter_map(|line| line.trim().parse().ok()).collect();
    classify(&interfaces, &wireguard_names(&interfaces), &tailnet)
}

// Sealed to each other machine on the account's list and signed with this machine's key, so a peer trusts an address only as this machine announced it.
pub fn announce(identity: &Identity, account: &AccountKey, client: &Client, cluster: &Cluster, announced: &Addresses, now: u64) -> Result<usize> {
    let mut sent = 0;
    for (machine, keys) in cluster.keys.iter().filter(|(machine, _)| **machine != identity.machine) {
        let envelope = cluster::seal_to(identity, account, machine, keys, cluster::new_id()?, &Payload::Addresses(announced.clone()), now)?;
        client.put::<_, serde::de::IgnoredAny>("/v1/m/addresses", &MessageUpload { envelope }).map_err(|error| hub_said(&error))?;
        sent += 1;
    }
    Ok(sent)
}

fn hub_said(error: &HubError) -> String {
    match error {
        HubError::Status { status: 404 | 405, .. } => "the hub keeps no machine addresses yet; it needs an update".to_string(),
        error => error.to_string(),
    }
}

// The addresses the account's other machines announced to this one, each kept only when it opens against its sender's key on the list.
pub fn announced(identity: &Identity, client: &Client, cluster: &Cluster, now: u64) -> Result<BTreeMap<Id, Addresses>> {
    let listed: AnnouncedAddresses = client.get("/v1/m/addresses").map_err(|error| hub_said(&error))?;
    let mut found = BTreeMap::new();
    for envelope in &listed.envelopes {
        let from = &envelope.header.from;
        let Some(keys) = cluster.keys.get(from) else { continue };
        if let Ok(Payload::Addresses(addresses)) = cluster::open_message(envelope, identity, keys, now) {
            found.insert(from.clone(), addresses);
        }
    }
    Ok(found)
}
