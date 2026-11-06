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
        #expect(peer.pageURL(port: 8080, byName: false)?.absoluteString == "http://127.0.0.1:52311/")
        #expect(peer.pageURL(port: 3000, byName: false)?.absoluteString == "http://127.0.0.1:3000/")
        #expect(peer.pageURL(port: 8080, byName: true)?.absoluteString == "http://mac.pai-sho:8080/")
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
