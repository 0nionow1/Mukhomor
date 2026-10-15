import { requireThat } from './http.mjs';

export function contentStore(db, transaction) {
  db.exec(`
    CREATE TABLE IF NOT EXISTS site_content (
      id INTEGER PRIMARY KEY CHECK(id=1), draft TEXT NOT NULL, revision INTEGER NOT NULL,
      published TEXT, published_revision INTEGER, published_at INTEGER
    ) STRICT;
    CREATE TABLE IF NOT EXISTS site_release_sources (version TEXT PRIMARY KEY, document TEXT NOT NULL) STRICT;
    CREATE TABLE IF NOT EXISTS site_history (revision INTEGER PRIMARY KEY, document TEXT NOT NULL, created INTEGER NOT NULL) STRICT;
    CREATE TABLE IF NOT EXISTS site_audit (id INTEGER PRIMARY KEY, action TEXT NOT NULL, revision INTEGER NOT NULL, created INTEGER NOT NULL) STRICT;
  `);
  const audit=(action,revision,time)=>{
    db.prepare('INSERT INTO site_audit(action,revision,created) VALUES (?,?,?)').run(action,revision,time);
    db.exec('DELETE FROM site_audit WHERE id NOT IN (SELECT id FROM site_audit ORDER BY id DESC LIMIT 100)');
  };
  const read=()=>db.prepare('SELECT * FROM site_content WHERE id=1').get();
  return {
    initContent(document,sources) {
      transaction(()=>{
        db.prepare('INSERT OR IGNORE INTO site_content(id,draft,revision) VALUES (1,?,1)').run(JSON.stringify(document));
        for(const [version,source] of sources) db.prepare('INSERT OR IGNORE INTO site_release_sources(version,document) VALUES (?,?)').run(version,JSON.stringify(source));
      });
    },
    contentSources() { return new Map(db.prepare('SELECT version,document FROM site_release_sources').all().map(row=>[row.version,JSON.parse(row.document)])); },
    addContentSource(source) {
      transaction(()=>{
        requireThat(db.prepare('SELECT count(*) AS n FROM site_release_sources').get().n<200 || db.prepare('SELECT 1 FROM site_release_sources WHERE version=?').get(source.version),409,'release_import_capacity');
        db.prepare('INSERT INTO site_release_sources(version,document) VALUES (?,?) ON CONFLICT(version) DO UPDATE SET document=excluded.document').run(source.version,JSON.stringify(source));
      });
    },
    getContent() {
      const row=read();
      return {document:JSON.parse(row.draft),revision:row.revision,publishedRevision:row.published_revision,publishedAt:row.published_at,
        history:db.prepare('SELECT revision,created FROM site_history ORDER BY revision DESC').all(),
        audit:db.prepare('SELECT action,revision,created FROM site_audit ORDER BY id DESC LIMIT 20').all()};
    },
    getPublishedContent() {
      const row=read();
      return row?.published ? {document:JSON.parse(row.published),revision:row.published_revision,publishedAt:row.published_at} : null;
    },
    saveContent(document,revision,time) {
      return transaction(()=>{
        requireThat(read().revision===revision,409,'content_conflict');
        const next=revision+1;
        db.prepare('UPDATE site_content SET draft=?,revision=? WHERE id=1').run(JSON.stringify(document),next);
        audit('save',next,time);return next;
      });
    },
    publishContent(revision,time) {
      transaction(()=>{
        const row=read();requireThat(row.revision===revision,409,'content_conflict');
        requireThat(row.published_revision!==revision,409,'already_published');
        db.prepare('UPDATE site_content SET published=draft,published_revision=revision,published_at=? WHERE id=1').run(time);
        db.prepare('INSERT INTO site_history(revision,document,created) VALUES (?,?,?)').run(revision,row.draft,time);
        db.exec('DELETE FROM site_history WHERE revision NOT IN (SELECT revision FROM site_history ORDER BY revision DESC LIMIT 10)');
        audit('publish',revision,time);
      });
    },
    restoreContent(revision,fromRevision,time) {
      return transaction(()=>{
        requireThat(read().revision===revision,409,'content_conflict');
        const previous=db.prepare('SELECT document FROM site_history WHERE revision=?').get(fromRevision);
        requireThat(previous,404,'revision_not_found');
        const next=revision+1;
        db.prepare('UPDATE site_content SET draft=?,revision=? WHERE id=1').run(previous.document,next);
        audit('restore',next,time);return next;
      });
    }
  };
}
