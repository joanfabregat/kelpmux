// WASSH's minimal filesystem interface; proof credentials contain no private key.
export class MemoryFileSystem {
  constructor(files = {}) {
    this.files = new Map(Object.entries(files));
    this.directories = new Set(["/", ".ssh", "/.ssh"]);
  }
  async createDirectory(path) {
    this.directories.add(path);
  }
  async details(path) {
    if (this.directories.has(path))
      return { type: "directory", createdAt: Date.now() };
    if (!this.files.has(path)) throw Error("File not found");
    return { type: "file", createdAt: Date.now() };
  }
  async readFile(path) {
    if (!this.files.has(path)) throw Error("File not found");
    return this.files.get(path);
  }
  async writeFile(path, value) {
    this.files.set(path, value);
  }
  async removeFile(path) {
    this.files.delete(path);
  }
  async copyFile(from, to) {
    this.files.set(to, await this.readFile(from));
  }
}
