import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


DEMO_DIR = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("showcase_generate", DEMO_DIR / "generate.py")
generate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generate)


class ShowcaseSourceTests(unittest.TestCase):
    def test_repository_showcase_declares_eight_valid_looks(self):
        looks = generate.load_showcase(DEMO_DIR)

        self.assertEqual(8, len(looks))
        self.assertEqual("01-soft-tape", looks[0].slug)
        self.assertEqual("08-meltdown", looks[-1].slug)

    def test_load_showcase_rejects_duplicate_slugs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "maps").mkdir()
            (root / "maps" / "same.toml").write_text("[audio]\n", encoding="utf-8")
            (root / "showcase.json").write_text(
                json.dumps(
                    {
                        "looks": [
                            {"slug": "same", "title": "One", "description": "First"},
                            {"slug": "same", "title": "Two", "description": "Second"},
                        ]
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "duplicate showcase slug: same"):
                generate.load_showcase(root)

    def test_load_showcase_requires_each_mapping(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "maps").mkdir()
            (root / "showcase.json").write_text(
                json.dumps(
                    {
                        "looks": [
                            {
                                "slug": "missing-map",
                                "title": "Missing",
                                "description": "No mapping exists",
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "missing mapping.*missing-map.toml"):
                generate.load_showcase(root)


class SiteVerificationTests(unittest.TestCase):
    def test_release_workflow_enables_cargo_about_cli(self):
        workflow = (
            DEMO_DIR.parent / ".github" / "workflows" / "release.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "cargo install --locked cargo-about --version 0.9.2 --features cli",
            workflow,
        )

    def test_release_workflow_writes_portable_checksums(self):
        workflow = (
            DEMO_DIR.parent / ".github" / "workflows" / "release.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "working-directory: dist\n        run: sha256sum -- *.tar.gz", workflow
        )
        self.assertNotIn("sha256sum dist/*.tar.gz", workflow)

    def test_styles_allow_video_grid_items_to_shrink_on_mobile(self):
        styles = (DEMO_DIR / "styles.css").read_text(encoding="utf-8")

        self.assertIn(".hero > *, .look-card, .reference-card > * { min-width: 0; }", styles)
        self.assertIn(".hero { grid-template-columns: minmax(0, 1fr);", styles)

    def test_verify_command_runs_after_all_verifiers_are_defined(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            site = root / "site"
            media = site / "media"
            posters = site / "posters"
            tools = root / "bin"
            media.mkdir(parents=True)
            posters.mkdir()
            tools.mkdir()
            slugs = [
                "00-clean-reference",
                *(look.slug for look in generate.load_showcase(DEMO_DIR)),
                "09-comparison-grid",
                "10-sampler-reel",
            ]
            for slug in slugs:
                (media / f"{slug}.mp4").touch()
                (posters / f"{slug}.jpg").touch()
            (site / "index.html").write_text("<!doctype html><title>fixture</title>")
            fake_probe = tools / "ffprobe"
            fake_probe.write_text(
                f"#!{sys.executable}\n"
                "import json, sys\n"
                "name = sys.argv[-1]\n"
                "sampler = '10-sampler-reel' in name\n"
                "comparison = '09-comparison-grid' in name\n"
                "seconds = 32 if sampler else 8\n"
                "print(json.dumps({'streams': ["
                "{'codec_type': 'video', 'width': 960, 'height': 624 if comparison else 540, 'nb_frames': str(seconds * 30)},"
                "{'codec_type': 'audio'}], 'format': {'duration': str(seconds)}}))\n",
                encoding="utf-8",
            )
            fake_probe.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = f"{tools}{os.pathsep}{env['PATH']}"

            result = subprocess.run(
                [
                    sys.executable,
                    DEMO_DIR / "generate.py",
                    "verify",
                    "--output",
                    site,
                ],
                cwd=DEMO_DIR.parent,
                env=env,
                capture_output=True,
                text=True,
            )

            self.assertEqual(0, result.returncode, result.stderr)
            self.assertIn("verified 11 videos", result.stdout)

    def test_assemble_site_copies_public_sources_and_renders_cards(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source"
            site = root / "site"
            (source / "maps").mkdir(parents=True)
            (source / "maps" / "01-look.toml").write_text("[audio]\n", encoding="utf-8")
            (source / "base.json").write_text("{}\n", encoding="utf-8")
            (source / "index.template.html").write_text(
                "<html><body>{{SHOWCASE_CARDS}}</body></html>", encoding="utf-8"
            )
            for filename in ("styles.css", "app.js", "favicon.svg"):
                (source / filename).write_text(filename, encoding="utf-8")
            looks = [generate.Look("01-look", "Look", "Description")]

            generate.assemble_site(source, site, looks)

            self.assertIn("01-look", (site / "index.html").read_text(encoding="utf-8"))
            self.assertTrue((site / "maps" / "01-look.toml").is_file())
            self.assertEqual("{}\n", (site / "assets" / "base.json").read_text())
            self.assertTrue((site / ".nojekyll").is_file())

    def test_assemble_site_can_preserve_rendered_media(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source"
            site = root / "site"
            (source / "maps").mkdir(parents=True)
            (source / "maps" / "01-look.toml").write_text("[audio]\n", encoding="utf-8")
            (source / "base.json").write_text("{}\n", encoding="utf-8")
            (source / "index.template.html").write_text(
                "<html><body>{{SHOWCASE_CARDS}}</body></html>", encoding="utf-8"
            )
            for filename in ("styles.css", "app.js", "favicon.svg"):
                (source / filename).write_text(filename, encoding="utf-8")
            (site / "media").mkdir(parents=True)
            (site / "posters").mkdir()
            (site / "media" / "render.mp4").write_bytes(b"video")
            (site / "posters" / "render.jpg").write_bytes(b"poster")

            generate.assemble_site(
                source,
                site,
                [generate.Look("01-look", "Look", "Description")],
                preserve_media=True,
            )

            self.assertEqual(b"video", (site / "media" / "render.mp4").read_bytes())
            self.assertEqual(b"poster", (site / "posters" / "render.jpg").read_bytes())

    def test_render_index_inserts_video_and_mapping_for_every_look(self):
        template = "<main>{{SHOWCASE_CARDS}}</main>"
        looks = [
            generate.Look("01-soft-tape", "Soft tape", "Gentle wobble."),
            generate.Look("02-bass-buckle", "Bass buckle", "Bass bends."),
        ]

        rendered = generate.render_index(template, looks)

        self.assertIn('data-src="media/01-soft-tape.mp4"', rendered)
        self.assertIn('href="maps/01-soft-tape.toml"', rendered)
        self.assertIn('data-src="media/02-bass-buckle.mp4"', rendered)
        self.assertNotIn("{{SHOWCASE_CARDS}}", rendered)

    def test_verify_local_links_reports_missing_targets(self):
        with tempfile.TemporaryDirectory() as tmp:
            site = Path(tmp)
            (site / "index.html").write_text(
                '<a href="maps/look.toml">map</a><video data-src="missing.mp4"></video>',
                encoding="utf-8",
            )
            (site / "maps").mkdir()
            (site / "maps" / "look.toml").write_text("[audio]\n", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "missing.mp4"):
                generate.verify_local_links(site)

    def test_enforce_size_limit_rejects_oversize_site(self):
        with tempfile.TemporaryDirectory() as tmp:
            site = Path(tmp)
            (site / "large.bin").write_bytes(b"12345")

            with self.assertRaisesRegex(ValueError, "5 bytes exceeds 4-byte site limit"):
                generate.enforce_size_limit(site, 4)


if __name__ == "__main__":
    unittest.main()
