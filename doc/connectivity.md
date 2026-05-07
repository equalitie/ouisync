# Ouisync connectivity

This document describes how Ouisync achieves connectivity between devices under various network
conditions and configurations. 

## Configurations

Here are some common network configuration that Ouisync may run on:

### Public

Device is on a public IP and Ouisync is listening on a port that's open to inbound connections

### Home

Regular home router connected to a ISP. Typically the router's firewall blocks any unsolicited
inbound connections and there is endpoint-independent NAT. If the router has working and enabled
UPnP or manually set up port forwarding, this configuration behaves the same as `public`.

### CG-NAT (Carrier-grade NAT)

Typically mobile networks or some ISPs. Similar to `home` but typically without UPnP or manual port
forwarding. Hairpinning might not be enabled.

### Symmetric (endpoint-dependent NAT mapping)

Typically used in some corporate networks or hotel, airport or similar WiFi network. Some home
routers and mobile carriers also do this.

### Blocked protocols

Some network might completely block UDP (except DNS). Some might further block most of TCP as well
except of few select ports - typically only `80` (HTTP) and `443` (HTTPS). 

## Connectivity matrix

Which configurations allow direct connection. If direct connection is impossible, the nodes can
still sync via a third node (typically a cache server, but any node would do as long as both nodes
can connect to it and share the same repository with it).

|             | `public` | `home` | `cgnat`    | `symmetric` | `blocked`  |   
|-------------|----------|--------|------------|-------------|------------|
| `public`    |  yes     | yes    | yes        | yes         | yes [^1]   |
| `home`      |          | yes    | yes        | no [^2]     | no [^3]    | 
| `cgnat`     |          |        | yes [^4]   | no [^2]     | no [^3]    |
| `symmetric` |          |        |            | no [^5]     | no         |  
| `blocked`   |          |        |            |             | no [^6]    |

[^1]: Works only if the `public` node listens on TCP. If all ports except HTTP/HTTPS are blocked,
the `public` node would have to listen on these ports. In case the blocking is based on deep packet
inspection, it would currently not work as Ouisync's TCP traffic doesn't look like HTTP/HTTPS.

[^2]: Currently doesn't work but is potentially solvable using the "birthday paradox" approach as
described in the [tailscale blog](https://tailscale.com/blog/how-nat-traversal-works).

[^3]: Currently doesn't work. Would require TCP hole punching which Ouisync currently doesn't
implement.

[^4]: This works except if both devices are behind the same NAT. In that case connection would
require hairpinning which CG-NATs typically don't enable (citation needed). Unclear whether local
discovery would work.

[^5]: Currently doesn't work. In theory, the "birthday paradox" method can be used here too, on both
nodes. In practice it would require many more probes than in the symmetric-asymmetric case which
could take too long and might result in NAT session exhaustion.

[^6]: Networks that block UDP typically use symmetric NAT (citation needed) and so the same
restrictions as for the symmetric-symmetric case applies here, in addition to requiring TCP hole
punching.

## Possible improvements

### "Birthday paradox"

The `home` - `symmetric` and `cgnat` - `symmetric` cases can be handled by implementing the "birthday
paradox" method described in the [tailscale
blog](https://tailscale.com/blog/how-nat-traversal-works). In short: the node behind the symmetric
NAT would create multiple UDP endpoints and run hole-punching against the other node from all of
them. The other node would run multiple connection attempts (including hole punching) each to a
different port (say chosen randomly). Tailscale claims that with 256 listeners on the symmetric
side, the probability of successful connection is 99.9% after 2048 connection attempts.

How would this be implemented in Ouisync?

First, nodes would need to detect what type of NAT they are behind. This can be done with STUN
queries or if that fails (e.g., no STUN servers are reachable) by abusing DHT[^7] (but that would be
less reliable). If the node determines it's behind symmetric NAT, it would create multiple QUIC
endpoints. When initiating outgoing connection attempt to a peer, it would do so from all of those
endpoints simultaneously. 

[^7]: It can work somehow like this: First, a unique info-hash is generated and the node announces
itself on it. Then it performs a lookup on the same info hash. If all the responses have the same
port, it indicates the node is behind endpoint-independent NAT. Otherwise its behind
endpoint-dependent. 

The other node would need to somehow learn that this node is behind symmetric NAT so that it can
initiate the port probing. One way this can be done is that the node behind symmetric NAT would
somehow encode that fact into its DHT announces. There are multiple ways to do this: it can encode
it using a special port value (say 0 or 1) or it can announce on a different info hash, say,
calculated as `SHA1(original_info_hash || "symmetric-nat")` (and lookups would have to be performed
for both info hashes).

Once the node knows the peer is behind symmetric NAT, it would start multiple connection attempts,
each one to a different port, until connection is established or until a timeout. To speed things
up, multiple connection attempts can be made concurrently, but care needs to be taken to not
overload the router or to not look like we are port-scanning.

### TCP hole punching

Ouisync supports TCP but currently it can only connect to a peer that is exposed to the internet
(`public` or `home` + port forwarding). To support connecting to peers behind NATs, we would need to
implement TCP hole punching. More research is needed about this. 

### HTTP/HTTPS

We can add HTTP and/or HTTPS as additional transport protocol to support the cases where non-HTTP
traffic is blocked. To support HTTP, we would need to encode Ouisync messages as HTTP request or
responses. Supporting HTTPS should be simpler - adding TLS on top of the TCP protocol we already
have and setting correct ALPN should be all that is needed for it to look like HTTPS.
