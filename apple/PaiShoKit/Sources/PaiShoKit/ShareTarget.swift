import Foundation

/// Where a site takes shares, from its web app manifest's `share_target`
/// (https://w3c.github.io/web-share-target/). A page names its manifest with
/// `<link rel="manifest" href="...">`; the manifest's `share_target` says
/// where to post and what each field is called. The share sheet offers only
/// the ports whose site has one, and sends there.
public struct ShareTarget: Codable, Equatable, Sendable {
    /// Where shares are posted, resolved against the manifest's address.
    public var action: URL
    /// The form field files go in.
    public var files: String
    /// The form fields for words, when the site takes them.
    public var title: String?
    public var text: String?

    public init(action: URL, files: String, title: String? = nil, text: String? = nil) {
        self.action = action
        self.files = files
        self.title = title
        self.text = text
    }

    /// The manifest a page links to, resolved against the page's address.
    public static func manifestURL(html: String, page: URL) -> URL? {
        let tags = html.matches(of: /<link\b[^>]*>/.ignoresCase()).map { String(html[$0.range]) }
        for tag in tags {
            let rel = attribute("rel", in: tag)?.lowercased().split(separator: " ") ?? []
            guard rel.contains("manifest"), let href = attribute("href", in: tag) else { continue }
            return URL(string: href, relativeTo: page)?.absoluteURL
        }
        return nil
    }

    /// The manifest's share target. Only one that posts files as a
    /// multipart form will do, as a share sheet sends files.
    public static func from(manifest: Data, at url: URL) -> ShareTarget? {
        guard let json = try? JSONSerialization.jsonObject(with: manifest) as? [String: Any],
              let target = json["share_target"] as? [String: Any],
              let actionPath = target["action"] as? String,
              (target["method"] as? String)?.uppercased() == "POST",
              (target["enctype"] as? String)?.lowercased() == "multipart/form-data",
              let params = target["params"] as? [String: Any],
              let action = URL(string: actionPath, relativeTo: url)?.absoluteURL
        else { return nil }
        let files: String?
        if let list = params["files"] as? [[String: Any]] {
            files = list.first?["name"] as? String
        } else {
            files = (params["files"] as? [String: Any])?["name"] as? String
        }
        guard let files else { return nil }
        return ShareTarget(action: action, files: files, title: params["title"] as? String, text: params["text"] as? String)
    }

    /// The form a share posts: each file under `files`, and the words, if
    /// the site takes them.
    public func form(files shared: [(name: String, type: String, data: Data)], text words: String? = nil,
                     boundary: String = "PaiSho-" + UUID().uuidString) -> (body: Data, contentType: String) {
        var body = Data()
        func add(_ s: String) { body.append(Data(s.utf8)) }
        if let words, !words.isEmpty, let field = text {
            add("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(field)\"\r\n\r\n\(words)\r\n")
        }
        for file in shared {
            let name = file.name.replacingOccurrences(of: "\"", with: "")
            add("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(files)\"; filename=\"\(name)\"\r\n")
            add("Content-Type: \(file.type)\r\n\r\n")
            body.append(file.data)
            add("\r\n")
        }
        add("--\(boundary)--\r\n")
        return (body, "multipart/form-data; boundary=\(boundary)")
    }

    private static func attribute(_ name: String, in tag: String) -> String? {
        let pattern = "\\b\(name)\\s*=\\s*(?:\"([^\"]*)\"|'([^']*)'|([^\\s>]+))"
        guard let re = try? NSRegularExpression(pattern: pattern, options: .caseInsensitive),
              let m = re.firstMatch(in: tag, range: NSRange(tag.startIndex..., in: tag))
        else { return nil }
        for i in 1...3 {
            if let r = Range(m.range(at: i), in: tag) { return String(tag[r]) }
        }
        return nil
    }
}
