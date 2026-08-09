# Downloading

Pick by what you're fetching: **media → yt-dlp**, **a plain file you can identify → curl/wget**, **genuinely unsure → yt-dlp is the safe first guess**. `yt-dlp`, `ffmpeg`, `curl` and `wget` are all already installed.

## Media (video, audio, anything streamable) — use yt-dlp

**`yt-dlp` is your default for any media URL. Just throw the URL at it.** It handles a huge range of sites and formats implicitly — YouTube, Twitter/X, TikTok, Instagram, SoundCloud, Reddit, direct `.mp4`/`.m3u8`/`.mpd` links, and thousands more — figuring out the right extractor, resolving playlists, and merging video+audio for you.

```bash
yt-dlp "URL"                          # best quality, auto format
yt-dlp -f "bestvideo+bestaudio/best" "URL"
yt-dlp -x --audio-format mp3 "URL"    # audio only, extract to mp3
yt-dlp -o "%(title)s.%(ext)s" "URL"   # clean output filename
```

Useful flags:
- `--list-formats` (`-F`) — see what's available before picking.
- `-f "b[height<=720]"` — cap resolution.
- `--playlist-items 1-5` — grab part of a playlist.
- `--cookies-from-browser firefox` — for content behind a login.
- `--write-sub --sub-lang en` — subtitles.

If a plain `yt-dlp URL` fails, update it first (`pip install -U yt-dlp`) — sites change and the newest version usually already has the fix — then retry with `-F` to inspect formats.

## Plain files (documents, archives, images, anything non-media)

If you already know it's a regular file — a PDF, image, zip, JSON, release binary, etc. — don't bother with yt-dlp; just grab it directly:

```bash
curl -L -o out.ext "URL"    # -L follows redirects
wget -O out.ext "URL"
```

- `-L` / redirects matter — many download links bounce through redirects.
- For large files, `wget -c` / `curl -C -` resume a partial download.

## Not sure what the URL is?

Then yt-dlp is the safe first shot — it covers far more than it looks like it should, and reports cleanly when a link has no extractable media. If it says there's nothing to extract, fall back to `curl -L`.
