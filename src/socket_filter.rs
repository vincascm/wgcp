use std::net::Ipv4Addr;

use libc::sock_filter;

pub fn broker_response_filter(src_ip: Ipv4Addr, src_port: u16, dst_port: u16) -> Vec<sock_filter> {
    vec![
        // ld [12], src ip
        sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 12,
        },
        // jeq src ip
        sock_filter {
            code: 0x15,
            jt: 0,
            jf: 5,
            k: src_ip.into(),
        },
        // ldh [20], src port
        sock_filter {
            code: 0x28,
            jt: 0,
            jf: 0,
            k: 20,
        },
        // jeq src port
        sock_filter {
            code: 0x15,
            jt: 0,
            jf: 3,
            k: src_port as u32,
        },
        // ldh [22], dst port
        sock_filter {
            code: 0x28,
            jt: 0,
            jf: 0,
            k: 22,
        },
        // jeq dst port
        sock_filter {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: dst_port as u32,
        },
        sock_filter {
            code: 0x6,
            jt: 0,
            jf: 0,
            k: u32::MAX,
        },
        sock_filter {
            code: 0x6,
            jt: 0,
            jf: 0,
            k: 0,
        },
    ]
}
