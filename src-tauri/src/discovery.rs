//! LAN discovery over mDNS/DNS-SD (`_bekdesk._tcp`). Advertised data is public; trust is decided by
//! the certificate fingerprint, which `net::open` re-checks on every connection.

use crate::app::{App, Peer};
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::Arc;

pub const SERVICE: &str = "_bekdesk._tcp.local.";

pub fn start(app: Arc<App>) -> Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new()?;
    let short = &app.id.fp[..16];
    let mut props = HashMap::new();
    props.insert("fp".to_string(), app.id.fp.clone());
    props.insert("name".to_string(), app.name().chars().take(60).collect());
    props.insert("os".to_string(), App::os().to_string());
    props.insert("mon".to_string(), app.monitors.lock().unwrap().chars().take(200).collect());
    let info = ServiceInfo::new(SERVICE, short, &format!("bk-{short}.local."), "", app.port, props)?.enable_addr_auto();
    mdns.register(info)?;

    let rx = mdns.browse(SERVICE)?;
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                ServiceEvent::ServiceResolved(info) => {
                    let Some(fp) = info.get_property_val_str("fp").map(str::to_string) else { continue };
                    if fp == app.id.fp || fp.len() != 64 {
                        continue;
                    }
                    let mut addrs: Vec<_> = info.get_addresses().iter().copied().collect();
                    // Routable IPv4 first; link-local addresses (169.254/fe80) are last resorts.
                    addrs.sort_by_key(|a| match a {
                        std::net::IpAddr::V4(v) => v.is_link_local() as u8,
                        std::net::IpAddr::V6(_) => 2,
                    });
                    let peer = Peer {
                        name: info.get_property_val_str("name").unwrap_or("Unknown").to_string(),
                        os: info.get_property_val_str("os").unwrap_or("").to_string(),
                        monitors: info.get_property_val_str("mon").unwrap_or("").to_string(),
                        addrs,
                        port: info.get_port(),
                        fullname: info.get_fullname().to_string(),
                        fp: fp.clone(),
                    };
                    app.stale.lock().unwrap().remove(&fp);
                    let new = app.peers.lock().unwrap().insert(fp, peer.clone()).is_none();
                    if new {
                        app.log(format!("found {} ({}) at {:?}", peer.name, peer.os, peer.addrs));
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    app.peers.lock().unwrap().retain(|_, p| p.fullname != fullname);
                }
                _ => {}
            }
        }
    });
    Ok(mdns)
}
