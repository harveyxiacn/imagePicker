import importlib.util
from pathlib import Path
import tempfile
import unittest
from datetime import datetime
from PIL import Image

spec = importlib.util.spec_from_file_location("library", Path(__file__).with_name("comfy-photo-library.py"))
lib = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lib)


class LibraryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        lib.ROOT.mkdir(parents=True, exist_ok=True)

    def test_inventory_dependencies_and_burst_metadata(self):
        jobs = lib.jobs()
        self.assertEqual(len(jobs) + 48, 1000)
        self.assertEqual(len({j['name'] for j in jobs}), len(jobs))
        seen = set()
        bursts = {}
        for j in jobs:
            self.assertTrue(set(j.get('refs', [])) <= seen)
            if j['kind'] == 'i2i':
                self.assertIn(j['name'].rsplit('-', 1)[0] + '-00', seen)
            seen.add(j['name'])
            if j.get('burst'):
                bursts.setdefault(j['burst'], []).append(j)
        self.assertEqual(len(bursts), 60)
        for frames in bursts.values():
            self.assertEqual(len({j['device_index'] for j in frames}), 1)
            for a, b in zip(frames, frames[1:]):
                self.assertAlmostEqual((datetime.fromisoformat(b['taken_at']) - datetime.fromisoformat(a['taken_at'])).total_seconds(), .4)

    def test_roundtrip_all_devices_and_edge_metadata(self):
        with tempfile.TemporaryDirectory(dir=lib.ROOT) as tmp:
            png, jpg = Path(tmp) / 'test.png', Path(tmp) / 'test.jpg'
            Image.new('RGB', (64, 48), '#baa987').save(png)
            j = lib.jobs()[0]
            for i, device in enumerate(lib.DEVICES):
                lib.export_jpeg(png, jpg, j, device_index=i)
                with Image.open(jpg) as im:
                    e = im.getexif().get_ifd(34665)
                    self.assertEqual(e[42036], device[2])
                    self.assertAlmostEqual(float(e[33437]), device[4], places=3)
                    self.assertAlmostEqual(float(e[37386]), device[3], places=3)
            lib.export_jpeg(png, jpg, j, 'no-exif')
            with Image.open(jpg) as im:
                self.assertEqual(len(im.getexif()), 0)
            lib.export_jpeg(png, jpg, j, 'missing-lens')
            with Image.open(jpg) as im:
                self.assertNotIn(42036, im.getexif().get_ifd(34665))
            lib.export_jpeg(png, jpg, j, 'clock-offset')
            with Image.open(jpg) as im:
                self.assertEqual(im.getexif().get_ifd(34665)[36867], '2026:10:01 08:02:00')

    def test_reference_paths_and_burst_conditioning(self):
        jobs = {j['name']: j for j in lib.jobs()}
        records = {a: {'server_image': f'custom/{a}_00007_.png'} for a in lib.v2.ACTORS}
        group = lib.graph(jobs['family-dinner-1'], records, 20)
        self.assertEqual(group['11']['inputs']['image'], 'custom/a1_00007_.png [output]')
        records['bs01-a1-00'] = {'server_image': 'custom/base_00009_.png'}
        burst = lib.graph(jobs['bs01-a1-01'], records, 20)
        self.assertEqual(burst['10']['inputs']['image'], 'custom/base_00009_.png [output]')
        self.assertEqual(burst['4']['inputs']['images.image_1'], ['10', 0])
        self.assertIn('both eyes fully closed', burst['4']['inputs']['prompt'].lower())
        self.assertEqual(burst['6']['inputs']['latent_image'], ['4', 2])
        self.assertNotIn('identity reference', burst['4']['inputs']['prompt'])


if __name__ == '__main__':
    unittest.main()
