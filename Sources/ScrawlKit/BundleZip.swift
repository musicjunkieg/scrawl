import Foundation

/// Packs a `<id>.scrawl` folder into a zip for a Scrawl receiver (`scrawl-server`'s
/// `POST /bundles`): one `<id>.scrawl/` folder, every file stored as is. The images and the
/// movie are compressed already, so deflating them again would only cost time. Each file is read
/// twice in 1 MB pieces (once for its checksum, once to copy it), so a long movie is never held
/// in memory.
enum BundleZip {
  struct Failure: Error, CustomStringConvertible {
    let description: String
  }

  /// Writes `folder`'s files (hidden ones left out) to `file`, in name order, and returns its size.
  static func write(folder: URL, to file: URL) throws -> Int64 {
    let root = folder.lastPathComponent
    let files = try contents(of: folder)
    guard files.count < 0xFFFF else { throw Failure(description: "The bundle has too many files to zip") }
    guard FileManager.default.createFile(atPath: file.path, contents: nil) else {
      throw CocoaError(.fileWriteUnknown)
    }
    let output = try FileHandle(forWritingTo: file)
    defer { try? output.close() }
    var central = Data()
    var offset: UInt64 = 0
    for (relative, url) in files {
      let name = Data("\(root)/\(relative)".utf8)
      let (crc, size) = try checksum(of: url)
      guard size < UInt64(UInt32.max), offset < UInt64(UInt32.max) else {
        throw Failure(description: "The bundle is too large to zip")
      }
      var local = Data()
      local.append(le32: 0x0403_4B50)
      local.append(entryFields(crc: crc, size: UInt32(size), nameLength: name.count))
      local.append(le16: 0) // extra field length
      local.append(name)
      try output.write(contentsOf: local)
      try copy(url, to: output)

      central.append(le32: 0x0201_4B50)
      central.append(le16: 20) // made by
      central.append(entryFields(crc: crc, size: UInt32(size), nameLength: name.count))
      central.append(le16: 0) // extra field length
      central.append(le16: 0) // comment length
      central.append(le16: 0) // disk
      central.append(le16: 0) // internal attributes
      central.append(le32: 0) // external attributes
      central.append(le32: UInt32(offset))
      central.append(name)
      offset += UInt64(local.count) + size
    }
    guard offset + UInt64(central.count) < UInt64(UInt32.max) else {
      throw Failure(description: "The bundle is too large to zip")
    }
    var end = Data()
    end.append(le32: 0x0605_4B50)
    end.append(le16: 0) // this disk
    end.append(le16: 0) // the directory's disk
    end.append(le16: UInt16(files.count))
    end.append(le16: UInt16(files.count))
    end.append(le32: UInt32(central.count))
    end.append(le32: UInt32(offset))
    end.append(le16: 0) // comment length
    try output.write(contentsOf: central)
    try output.write(contentsOf: end)
    try output.synchronize()
    return Int64(offset) + Int64(central.count) + Int64(end.count)
  }

  /// The fields a local header and a central directory entry share, from "version needed" to the
  /// name's length: stored, UTF-8 names, dated 1980-01-01.
  private static func entryFields(crc: UInt32, size: UInt32, nameLength: Int) -> Data {
    var fields = Data()
    fields.append(le16: 20) // version needed
    fields.append(le16: 0x0800) // UTF-8 names
    fields.append(le16: 0) // stored
    fields.append(le16: 0) // time
    fields.append(le16: 0x0021) // date
    fields.append(le32: crc)
    fields.append(le32: size) // packed
    fields.append(le32: size) // unpacked
    fields.append(le16: UInt16(nameLength))
    return fields
  }

  /// Every file under `folder`, as its path inside it and its URL, in name order.
  private static func contents(of folder: URL) throws -> [(String, URL)] {
    // Paths relative to the folder, whatever symlinks its own path goes through (/var on iOS).
    guard let walker = FileManager.default.enumerator(atPath: folder.path) else {
      throw CocoaError(.fileReadNoSuchFile)
    }
    var files: [(String, URL)] = []
    while let relative = walker.nextObject() as? String {
      guard !relative.split(separator: "/").contains(where: { $0.hasPrefix(".") }),
            walker.fileAttributes?[.type] as? FileAttributeType == .typeRegular
      else { continue }
      files.append((relative, folder.appending(path: relative)))
    }
    return files.sorted { $0.0 < $1.0 }
  }

  private static func checksum(of url: URL) throws -> (UInt32, UInt64) {
    let input = try FileHandle(forReadingFrom: url)
    defer { try? input.close() }
    var crc = CRC32()
    var size: UInt64 = 0
    while let chunk = try input.read(upToCount: 1 << 20), !chunk.isEmpty {
      crc.update(chunk)
      size += UInt64(chunk.count)
    }
    return (crc.value, size)
  }

  private static func copy(_ url: URL, to output: FileHandle) throws {
    let input = try FileHandle(forReadingFrom: url)
    defer { try? input.close() }
    while let chunk = try input.read(upToCount: 1 << 20), !chunk.isEmpty {
      try output.write(contentsOf: chunk)
    }
  }
}

/// The zip format's checksum (CRC-32, the IEEE polynomial).
struct CRC32 {
  private static let table: [UInt32] = (0 ..< 256).map { byte in
    (0 ..< 8).reduce(UInt32(byte)) { crc, _ in crc & 1 == 1 ? 0xEDB8_8320 ^ (crc >> 1) : crc >> 1 }
  }

  private var crc: UInt32 = 0xFFFF_FFFF

  mutating func update(_ data: Data) {
    for byte in data {
      crc = Self.table[Int((crc ^ UInt32(byte)) & 0xFF)] ^ (crc >> 8)
    }
  }

  var value: UInt32 { crc ^ 0xFFFF_FFFF }
}

private extension Data {
  mutating func append(le16 value: UInt16) {
    Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
  }

  mutating func append(le32 value: UInt32) {
    Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
  }
}
