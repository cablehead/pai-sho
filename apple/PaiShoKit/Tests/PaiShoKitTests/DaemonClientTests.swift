import Foundation
import Testing
@testable import PaiShoKit

@Suite struct ListReply {
    @Test func aPortOpensWhereItIsBound() throws {
        // An iOS daemon: every peer at 127.0.0.1, port 8080 taken, so it
        // listens on another.
        let reply = """
        {"List":{"me":"m","daemon":"0.5.4-dev","peers":[{"key":"abcdefgh1234","name":"mac",\
        "online":true,"admission":"code","they_expose":[8080,3000],"ip":"127.0.0.1",\
        "bound":[3000,8080],"bound_at":[{"port":3000,"at":"127.0.0.1:3000"},\
        {"port":8080,"at":"127.0.0.1:52311"}]}],"i_expose":[],"grants":[]}}
        """
        let peers = try DaemonClient.peers(fromListReply: Data(reply.utf8))
        let peer = try #require(peers.first)
        #expect(peer.ports == [8080, 3000])
        #expect(peer.pageURL(port: 8080, byName: false)?.absoluteString == "http://mac.localhost:52311/")
        #expect(peer.pageURL(port: 3000, byName: false)?.absoluteString == "http://mac.localhost:3000/")
        #expect(peer.pageURL(port: 8080, byName: true)?.absoluteString == "http://mac.pai-sho:8080/")
    }

    @Test func aPageThatFailsOverHttpIsTriedOverHttps() throws {
        // A peer serving TLS on the port, such as an n2 board on 3042.
        let url = try #require(URL(string: "http://mac.localhost:3042/"))
        #expect(Peer.secureRetry(for: url)?.absoluteString == "https://mac.localhost:3042/")
        let secure = try #require(URL(string: "https://mac.localhost:3042/"))
        #expect(Peer.secureRetry(for: secure) == nil)
    }

    @Test func aPortNotBoundYetHasNoPage() throws {
        let reply = """
        {"List":{"me":"m","peers":[{"key":"k","name":null,"online":false,"admission":"code",\
        "they_expose":[80],"ip":"127.0.0.1","bound":[],"bound_at":[]}],"i_expose":[],"grants":[]}}
        """
        let peer = try #require(try DaemonClient.peers(fromListReply: Data(reply.utf8)).first)
        #expect(peer.pageURL(port: 80, byName: false) == nil)
    }

    @Test func anOlderDaemonOpensAtItsSurface() throws {
        // No bound_at: a port listens at the surface ip and its own number.
        let reply = """
        {"List":{"me":"m","peers":[{"key":"k","name":null,"online":true,"admission":"code",\
        "they_expose":[80],"ip":"127.0.1.2","bound":[80]}],"i_expose":[],"grants":[]}}
        """
        let peer = try #require(try DaemonClient.peers(fromListReply: Data(reply.utf8)).first)
        #expect(peer.boundAt == nil)
        #expect(peer.pageURL(port: 80, byName: true)?.absoluteString == "http://127.0.1.2:80/")
    }

    @Test func aPeerThatSaidGoodbyeIsAsleep() throws {
        let reply = """
        {"List":{"me":"m","peers":[\
        {"key":"a","name":"phone","online":false,"asleep":true,"admission":"code","they_expose":[],"ip":null,"bound":[]},\
        {"key":"b","name":"old","online":false,"admission":"code","they_expose":[],"ip":null,"bound":[]}\
        ],"i_expose":[],"grants":[]}}
        """
        let peers = try DaemonClient.peers(fromListReply: Data(reply.utf8))
        #expect(peers.first { $0.key == "a" }?.asleep == true)
        // A daemon older than `asleep` leaves it out.
        #expect(peers.first { $0.key == "b" }?.asleep == false)
    }

    @Test func aPeerBeingRedialedIsDialing() throws {
        let reply = """
        {"List":{"me":"m","peers":[\
        {"key":"a","name":"mac","online":false,"dialing":true,"admission":"code","they_expose":[],"ip":null,"bound":[]},\
        {"key":"b","name":"old","online":false,"admission":"code","they_expose":[],"ip":null,"bound":[]}\
        ],"i_expose":[],"grants":[]}}
        """
        let peers = try DaemonClient.peers(fromListReply: Data(reply.utf8))
        #expect(peers.first { $0.key == "a" }?.dialing == true)
        // A daemon older than `dialing` leaves it out.
        #expect(peers.first { $0.key == "b" }?.dialing == false)
    }

    @Test func aSleepReplyIsOkOrRefused() throws {
        try DaemonClient.ok(fromReply: Data(#""Ok""#.utf8))
        #expect(throws: DaemonError.refused("no")) {
            try DaemonClient.ok(fromReply: Data(#"{"Error":"no"}"#.utf8))
        }
    }

    @Test func onlinePeersComeFirst() throws {
        let reply = """
        {"List":{"me":"m","peers":[\
        {"key":"a","name":"a","online":false,"admission":"code","they_expose":[],"ip":null,"bound":[]},\
        {"key":"b","name":"b","online":true,"admission":"code","they_expose":[],"ip":null,"bound":[]}\
        ],"i_expose":[],"grants":[]}}
        """
        let peers = try DaemonClient.peers(fromListReply: Data(reply.utf8))
        #expect(peers.map(\.key) == ["b", "a"])
    }

    @Test func anErrorIsRefused() {
        #expect(throws: DaemonError.refused("nope")) {
            try DaemonClient.peers(fromListReply: Data(#"{"Error":"nope"}"#.utf8))
        }
    }

    @Test func anInviteReplyIsTheInvitation() throws {
        let invite = try DaemonClient.invitation(fromInviteReply: Data(#"{"Invite":"abc.def"}"#.utf8))
        #expect(invite == "abc.def")
        #expect(throws: DaemonError.refused("no")) {
            try DaemonClient.invitation(fromInviteReply: Data(#"{"Error":"no"}"#.utf8))
        }
    }
}

@Suite struct PeerPage {
    func peer(_ name: String?, key: String = "abcdefgh1234", at: String = "127.0.0.1:52311") -> Peer {
        Peer(key: key, name: name, online: true, ports: [8080], ip: "127.0.0.1", boundAt: [8080: at])
    }

    @Test func aNameBecomesOneDnsLabel() {
        #expect(peer("mac").hostLabel == "mac")
        #expect(peer("Andy's MacBook Pro").hostLabel == "andy-s-macbook-pro")
        #expect(peer("  --rust_dev--  ").hostLabel == "rust-dev")
        #expect(peer("Caf\u{E9}").hostLabel == "cafe")
        #expect(peer(String(repeating: "a", count: 70)).hostLabel.count == 63)
    }

    @Test func aPeerWithNoUsableNameIsNamedByItsKey() {
        #expect(peer(nil).hostLabel == "abcdefgh")
        #expect(peer("").hostLabel == "abcdefgh")
        #expect(peer("\u{1F600}!").hostLabel == "abcdefgh")
    }

    @Test func twoPeersWithOneNameGetTwoSites() {
        let first = peer("mac", key: "aaaaaaaa1111")
        let second = peer("Mac", key: "bbbbbbbb2222")
        let other = peer("linux", key: "cccccccc3333")
        let peers = [first, second, other]
        #expect(first.hostLabel(among: peers) == "mac-aaaaaaaa")
        #expect(second.hostLabel(among: peers) == "mac-bbbbbbbb")
        #expect(other.hostLabel(among: peers) == "linux")
    }

    @Test func aPortAtLocalhostOpensAtTheLabel() {
        let mac = peer("mac")
        #expect(mac.pageURL(port: 8080, byName: false)?.absoluteString == "http://mac.localhost:52311/")
        #expect(mac.pageURL(port: 8080, byName: false, label: "mac-abcdefgh")?.absoluteString
            == "http://mac-abcdefgh.localhost:52311/")
        // A Mac opens by name, through the daemon's resolver.
        #expect(mac.pageURL(port: 8080, byName: true)?.absoluteString == "http://mac.pai-sho:8080/")
    }

    @Test func aPortAtASurfaceAddressOpensThere() {
        // *.localhost reaches only 127.0.0.1, so a surface address stays as is.
        let mac = peer("mac", at: "127.0.1.2:8080")
        #expect(mac.pageURL(port: 8080, byName: false)?.absoluteString == "http://127.0.1.2:8080/")
    }

    @Test func thePageShowsThePeersOwnAddress() throws {
        let mac = peer("mac")
        #expect(mac.displayAddress(port: 8080) == "mac.pai-sho:8080")
        let shown = try #require(URL(string: "https://mac.localhost:52311/"))
        #expect(mac.displayURL(shown, port: 8080) == "https://mac.pai-sho:8080/")
        #expect(peer(nil).displayAddress(port: 8080) == "abcdefgh.pai-sho:8080")
    }
}

@Suite struct CertificateTrust {
    let opened = URL(string: "http://mac.localhost:52311/")!
    let serverTrust = NSURLAuthenticationMethodServerTrust

    @Test func theAddressTheAppOpenedIsTrusted() {
        #expect(Peer.trustsCertificate(method: serverTrust, host: "mac.localhost", port: 52311, opened: opened))
        #expect(Peer.trustsCertificate(method: serverTrust, host: "MAC.localhost", port: 52311, opened: opened))
    }

    @Test func anotherHostIsNotTrusted() {
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "linux.localhost", port: 52311, opened: opened))
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "127.0.0.1", port: 52311, opened: opened))
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "localhost", port: 52311, opened: opened))
    }

    @Test func anotherPortIsNotTrusted() {
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "mac.localhost", port: 52312, opened: opened))
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "mac.localhost", port: 443, opened: opened))
    }

    @Test func aSubresourceOnAnotherHostIsNotTrusted() {
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "cdn.example.com", port: 443, opened: opened))
    }

    @Test func aPageOpenedByNameIsNotTrusted() throws {
        // A Mac reaches mac.pai-sho through DNS, so its certificate is checked.
        let byName = try #require(URL(string: "http://mac.pai-sho:3042/"))
        #expect(!Peer.trustsCertificate(method: serverTrust, host: "mac.pai-sho", port: 3042, opened: byName))
    }

    @Test func onlyServerTrustIsAnswered() {
        #expect(!Peer.trustsCertificate(
            method: NSURLAuthenticationMethodHTTPBasic, host: "mac.localhost", port: 52311, opened: opened
        ))
    }
}

@Suite struct PageTitles {
    @Test func aPortKeyRoundTrips() throws {
        let id = PortID(peer: "abc123", port: 3042)
        #expect(id.storageKey == "abc123:3042")
        #expect(PortID(storageKey: id.storageKey) == id)
        #expect(PortID(storageKey: "nope") == nil)
        #expect(PortID(storageKey: ":3042") == nil)
    }

    @MainActor @Test func aTitleIsRememberedAcrossLaunches() throws {
        let defaults = try #require(UserDefaults(suiteName: "PageTitles-\(UUID())"))
        let client = DaemonClient(socketPath: "/nonexistent")
        let id = PortID(peer: "abc123", port: 3042)

        let first = PeerStore(client: client, opensByName: false, defaults: defaults)
        first.remember(title: "  n2 board ", for: id)
        first.remember(title: "", for: PortID(peer: "abc123", port: 1))
        #expect(first.pageTitles[id] == "n2 board")

        let next = PeerStore(client: client, opensByName: false, defaults: defaults)
        #expect(next.pageTitles == [id: "n2 board"])
    }

    /// On launch the app's pai-sho takes a moment to start. Until it answers,
    /// the peers it listed last time are shown, as starting.
    @MainActor @Test func knownPeersShowWhilePaiShoStarts() async throws {
        let defaults = try #require(UserDefaults(suiteName: "KnownPeers-\(UUID())"))
        defaults.set([["key": "abc123", "name": "mac", "ports": [3042]]], forKey: PeerStore.knownKey)
        let store = PeerStore(client: DaemonClient(socketPath: "/nonexistent"), opensByName: false, defaults: defaults)
        let mac = try #require(store.peers.first)
        #expect(mac.title == "mac" && mac.ports == [3042] && !mac.online)
        #expect(store.status(of: mac) == .starting)

        await store.refresh()
        #expect(store.peers.map(\.key) == ["abc123"])
        #expect(store.problem != nil)
        store.processRunning = false
        #expect(store.status(of: mac) == .stopped)
    }

    /// A peer that hasn't connected yet keeps the ports it had, greyed out,
    /// instead of showing none and then snapping them in.
    @Test func aPeerKeepsItsPortsUntilItConnects() {
        let known = [Peer(key: "a", ports: [3042, 3043]), Peer(key: "b", ports: [80])]
        let listed = [
            Peer(key: "a", dialing: true),
            Peer(key: "b", online: true, ports: [8080]),
            Peer(key: "c"),
        ]
        let shown = PeerStore.keepingPorts(of: known, in: listed)
        #expect(shown.map(\.ports) == [[3042, 3043], [8080], []])
        #expect(shown[0].address(port: 3042) == nil)
    }

    /// A link that opens a new tab opens a peer's page in place, and
    /// anything else in the system browser.
    @Test func aNewTabLinkToAPeerOpensHere() {
        let board = URL(string: "http://chunker.localhost:3042/#t-1793")
        #expect(WebView.Coordinator.opensHere(URL(string: "http://chunker.localhost:3042/local/web-push/")!, from: board))
        #expect(WebView.Coordinator.opensHere(URL(string: "http://hermes.localhost:3043/")!, from: board))
        #expect(WebView.Coordinator.opensHere(URL(string: "https://chunker.pai-sho:3042/")!, from: nil))
        #expect(!WebView.Coordinator.opensHere(URL(string: "https://github.com/cablehead")!, from: board))
    }

    /// A load that fails while pai-sho restarts is retried at its own
    /// address, not the port's front page.
    @Test func aFailedLoadIsRetriedWhereItWas() {
        let page = URL(string: "http://chunker.localhost:3042/local/web-push/")!
        let error = NSError(domain: NSURLErrorDomain, code: NSURLErrorCannotConnectToHost,
                            userInfo: [NSURLErrorFailingURLErrorKey: page])
        #expect(WebView.Coordinator.failedURL(error) == page)
        #expect(WebView.Coordinator.failedURL(NSError(domain: NSURLErrorDomain, code: -1)) == nil)
    }
}

@Suite struct LocalhostProxyRoutes {
    @Test func aPeerNameGoesToLoopback() {
        #expect(LocalhostProxy.target("chunker.localhost") == "127.0.0.1")
        #expect(LocalhostProxy.target("Mac-Abcdefgh.LOCALHOST") == "127.0.0.1")
        #expect(LocalhostProxy.target("localhost") == "127.0.0.1")
        #expect(LocalhostProxy.target("chunker.localhost.") == "127.0.0.1")
    }

    @Test func anyOtherNameGoesWhereItSays() {
        #expect(LocalhostProxy.target("example.com") == "example.com")
        #expect(LocalhostProxy.target("localhost.example.com") == "localhost.example.com")
        #expect(LocalhostProxy.target("notlocalhost") == "notlocalhost")
    }

    @Test func connectTargetsParse() {
        #expect(LocalhostProxy.hostPort("chunker.localhost:3042", defaultPort: 443).map { "\($0.0) \($0.1)" } == "chunker.localhost 3042")
        #expect(LocalhostProxy.hostPort("example.com", defaultPort: 443).map { "\($0.0) \($0.1)" } == "example.com 443")
        #expect(LocalhostProxy.hostPort("[::1]:8080", defaultPort: 443).map { "\($0.0) \($0.1)" } == "::1 8080")
        #expect(LocalhostProxy.hostPort(":80", defaultPort: 443) == nil)
        #expect(LocalhostProxy.hostPort("host:notaport", defaultPort: 443) == nil)
    }
}
