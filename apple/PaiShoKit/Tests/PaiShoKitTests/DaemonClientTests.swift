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
