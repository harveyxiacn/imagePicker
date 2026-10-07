#!/usr/bin/env python3
"""Synthetic evaluation set v2 (~1000 images) with local ComfyUI + Qwen-Image 2.1.

Compared with comfy-testset.py (v1):
  * 8 fixed "actors" with a reference portrait each; every person photo is generated with the
    actor's portrait as a visual reference (`TextEncodeQwenImage21` images.image_N), so people
    clustering / face search / per-person best shot have stable identities across scenes.
  * Real bursts: a base frame is generated once, then 4-6 frames are derived by img2img with a
    low denoise (expression / blink / small motion changes only) so frame pHash distances fall
    in the real-burst range.
  * Extra categories: street / architecture, objects / food / pets / documents / screens.

Phases (all resumable through manifest.json; --only limits to a phase):
  actors   8 reference portraits                       (t2i)
  people   singles: 8 actors x 25 scenes               (ref)
  groups   2-8 actors per photo, 30 scenes x 4         (ref)
  bursts   60 bases (ref) + ~300 derived frames        (i2i)
  places   landscapes / street / architecture, 40 x 3  (t2i)
  things   objects / food / pets / documents / screens (t2i)
  defects  12 scenes x 5 defect kinds                  (t2i)

Usage:
  scripts/comfy-testset-v2.py [--only people,bursts] [--limit N] [--dry-run] [--pilot]
  then: uv run --with pillow --with piexif scripts/comfy-testset-exif.py --src .../imagepicker-testset-v2 --dst .../imagepicker-testset-v2-jpg

Workflow JSONs (API format, loadable in the ComfyUI UI) are written by --save-workflows to
<ComfyUI>/user/default/workflows/imagepicker-testset/.
"""
from __future__ import annotations

import argparse
import json
import random
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

OUT = "imagepicker-testset-v2"
UNET = "qwen-image-2.1-Q8_0.gguf"
CLIP = "qwen3vl_8b_int8_convrot.safetensors"
VAE = "qwen_image_2.1_vae_bf16.safetensors"
STYLE = (
    "Realistic candid photograph shot on a modern mirrorless camera, natural skin texture, "
    "true-to-life colours, no text, no watermark, no logos."
)
SIZES = {"3:2": (1536, 1024), "2:3": (1024, 1536), "16:9": (1600, 896), "1:1": (1216, 1216), "4:3": (1408, 1056)}

# ----------------------------------------------------------------------------------------- actors
ACTORS = {
    "a1": "a Chinese woman in her late twenties with shoulder-length straight black hair, a small mole under her left eye, light makeup",
    "a2": "a Chinese man around 35 with short neat black hair, thin-framed rectangular glasses and a slight stubble",
    "a3": "a Chinese girl about 7 years old with two pigtails, round cheeks and a missing front tooth",
    "a4": "a Chinese grandmother in her late sixties with short grey permed hair, warm wrinkled smile, pearl earrings",
    "a5": "a tall Black man around 30 with a short fade haircut, full beard and a gap between his front teeth",
    "a6": "a white woman around 40 with wavy auburn hair tied back, freckles and green eyes",
    "a7": "a South Asian man in his early twenties with wavy dark hair, a prominent nose and a thin moustache",
    "a8": "a Chinese boy about 12 with a bowl haircut, thick black eyebrows and braces on his teeth",
}
ACTOR_WARDROBE = {
    "a1": ["a cream linen shirt", "a navy knit sweater", "a yellow sundress", "a black puffer jacket"],
    "a2": ["a grey polo shirt", "a dark blue suit jacket over a white shirt", "a green outdoor fleece", "a plain white t-shirt"],
    "a3": ["a pink dress with white dots", "a red hoodie", "a school uniform with a blue bow", "a yellow raincoat"],
    "a4": ["a floral blouse", "a burgundy cardigan", "a beige quilted coat", "a light blue qipao"],
    "a5": ["a mustard sweater", "a denim jacket", "a white dress shirt", "a black athletic t-shirt"],
    "a6": ["a striped breton top", "an olive trench coat", "a lavender blouse", "a grey hoodie"],
    "a7": ["a maroon kurta", "a black bomber jacket", "a teal t-shirt", "a checked flannel shirt"],
    "a8": ["a blue and white football jersey", "a grey school sweater", "an orange windbreaker", "a dinosaur t-shirt"],
}

# ----------------------------------------------------------------------------------------- scenes
SINGLE_SCENES = [
    ("park-golden", "standing in a city park at golden hour, warm backlight on the hair, shallow depth of field, smiling at the camera", "2:3"),
    ("window-read", "sitting by a large window reading a book, soft side light, wooden interior", "3:2"),
    ("beach-walk", "walking along the waterline on a sandy beach in late afternoon sun", "3:2"),
    ("backlit-sunset", "standing against the setting sun with strong rim light and lens flare, face still readable", "2:3"),
    ("highkey-white", "high-key studio portrait against a pure white seamless background, even bright light, background blown to white", "2:3"),
    ("candle-dim", "in a dim restaurant lit only by a candle, warm low light, visible grain", "2:3"),
    ("snow-slope", "on a snow-covered slope at noon in a ski jacket, bright white snow and blue sky", "3:2"),
    ("neon-alley", "at night in a narrow alley lit by neon signs, magenta and cyan light, wet pavement", "2:3"),
    ("noon-market", "under harsh midday sun at an outdoor market, hard shadows across the face", "3:2"),
    ("rain-umbrella", "holding a clear umbrella in light rain on a city street, overcast cool light", "2:3"),
    ("cafe-window", "seen through a cafe window holding a coffee cup, reflections of the street in the glass", "3:2"),
    ("temple-steps", "on the stone steps of an old Chinese temple with red pillars, soft overcast light", "2:3"),
    ("kitchen-cook", "cooking at a home kitchen stove, steam rising, warm tungsten light", "3:2"),
    ("office-desk", "at an office desk with a laptop and a plant, bright daylight from the side", "3:2"),
    ("bike-lane", "riding a city bike along a tree-lined lane, slight motion, dappled light", "3:2"),
    ("museum-hall", "looking at a painting in a museum hall, cool even gallery light", "2:3"),
    ("rooftop-city", "on a rooftop at blue hour with city lights behind, slight bokeh", "3:2"),
    ("forest-trail", "hiking on a forest trail with a backpack, green dappled light", "2:3"),
    ("lantern-night", "at a night market under red paper lanterns, warm mixed light", "2:3"),
    ("train-window", "sitting by a train window with the landscape blurred outside, soft daylight", "3:2"),
    ("pool-side", "sitting at the edge of a swimming pool with feet in the water, bright sun", "3:2"),
    ("library-shelf", "between tall library shelves holding a book, quiet warm light", "2:3"),
    ("balcony-flowers", "on an apartment balcony with potted flowers, soft morning light", "2:3"),
    ("bus-stop", "waiting at a bus stop on a grey rainy day, flat light, slight drizzle", "3:2"),
    ("garden-tea", "sitting in a garden drinking tea at a small table, afternoon shade", "3:2"),
]
GROUP_SCENES = [
    ("family-dinner", ["a1", "a2", "a3", "a4"], "around a dinner table at home, all looking at the camera and smiling, warm indoor light, dishes on the table", "3:2"),
    ("summit", ["a2", "a5", "a6", "a7"], "in hiking gear posing on a mountain summit, arms around each other, sunny, valley far below", "3:2"),
    ("two-selfie", ["a1", "a6"], "taking a selfie-style photo at arm's length in front of a Ferris wheel, grinning, wide angle", "3:2"),
    ("kids-hose", ["a3", "a8"], "playing with a garden hose in a backyard, water droplets in the air, afternoon sun", "3:2"),
    ("birthday", ["a1", "a2", "a3", "a4", "a8"], "around a birthday cake with lit candles in a dim living room, faces lit by candlelight", "3:2"),
    ("office-team", ["a2", "a5", "a6", "a7", "a1"], "standing in a bright modern office, casual business clothes, friendly smiles", "3:2"),
    ("grandma-park", ["a4", "a3"], "on a park bench, the child holding a balloon, warm evening light", "3:2"),
    ("temple-gate", ["a1", "a2", "a3", "a4", "a8"], "posing in front of a temple gate, four look at the camera while the boy on the right looks away at his phone", "3:2"),
    ("picnic", ["a5", "a6", "a3", "a8"], "at a picnic on the grass with a blanket and a basket, relaxed", "3:2"),
    ("stairs-formal", ["a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"], "arranged on the stone steps of an old building, formal group photo, even overcast light, every face visible", "3:2"),
    ("wedding", ["a1", "a5", "a6", "a7", "a2", "a4"], "a wedding party on a lawn, the bride in white and the groom in a dark suit in the middle, formal pose", "3:2"),
    ("cafe-table", ["a6", "a7", "a1", "a2"], "at a cafe table with coffee cups, chatting and laughing, window light", "3:2"),
    ("beach-run", ["a3", "a8", "a1"], "running along the beach toward the camera, splashing water, low sun", "3:2"),
    ("couple-torii", ["a1", "a2"], "walking hand in hand through a tunnel of vermilion torii gates, turning toward the camera", "3:2"),
    ("toast", ["a5", "a6", "a7", "a2"], "raising wine glasses in a toast at a restaurant table, warm tungsten light", "3:2"),
    ("karaoke", ["a1", "a7", "a8"], "singing into a microphone in a karaoke room with coloured lights", "3:2"),
    ("airport", ["a2", "a3", "a4"], "with suitcases at an airport departure hall, bright diffuse light", "3:2"),
    ("graduation", ["a7", "a5", "a6"], "in graduation gowns throwing caps in the air on a campus lawn", "3:2"),
    ("night-market", ["a1", "a3", "a8"], "eating skewers at a night market stall under warm lights", "3:2"),
    ("cycling", ["a2", "a8"], "on bicycles side by side on a river path, bright day", "3:2"),
    ("classroom", ["a3", "a8", "a6"], "at a classroom desk with crayons and paper, the adult helping", "3:2"),
    ("hotpot", ["a1", "a2", "a4", "a5", "a7"], "around a steaming hotpot in a busy restaurant, chopsticks in hand", "3:2"),
    ("playground", ["a3", "a4"], "the grandmother pushing the child on a swing, dappled light", "3:2"),
    ("bbq", ["a5", "a6", "a2", "a1"], "around a backyard barbecue grill with smoke, late afternoon", "3:2"),
    ("museum-group", ["a6", "a7", "a8"], "in front of a dinosaur skeleton in a museum, pointing up", "3:2"),
    ("boat", ["a1", "a2", "a3"], "on a small wooden boat on a calm lake, life jackets, overcast", "3:2"),
    ("snow-family", ["a2", "a3", "a8", "a4"], "building a snowman in a snowy garden, bright white snow", "3:2"),
    ("concert", ["a5", "a7", "a1"], "in a concert crowd with stage lights behind, phones raised, dark", "3:2"),
    ("market-stall", ["a4", "a1"], "at a vegetable market stall choosing produce, morning light", "3:2"),
    ("stadium", ["a2", "a8", "a5"], "in stadium seats wearing team scarves, cheering, floodlights", "3:2"),
]
BURST_FRAMES = [
    ("eyes open, natural smile, looking at the camera, sharp focus", ["best"]),
    ("both eyes closed mid-blink, otherwise identical", ["eyes-closed"]),
    ("mouth half open mid-sentence, eyes slightly squinted", ["mid-talk"]),
    ("looking slightly away from the camera to the side", ["gaze-away"]),
    ("slight motion blur on the head and hands from movement, background sharp", ["blur"]),
    ("bigger laugh, head tilted back a little", ["laugh"]),
]
BURST_SINGLE_ACTIONS = [
    "jumping in the air on a beach at sunset with arms raised",
    "blowing out candles on a birthday cake in a dim room",
    "throwing a frisbee in a park on a bright afternoon",
    "walking through a tunnel of vermilion torii gates, glancing back",
    "on a playground swing at the top of its arc under trees",
    "playing an acoustic guitar on a cobblestone square",
    "twirling in a long dress on a sunlit lawn",
    "running toward the camera on a forest trail",
    "stepping off a bus and waving at the camera",
    "skipping stones at a calm lake shore in evening light",
    "stirring a wok in a home kitchen with steam rising",
    "posing in front of a fountain in a city square at noon",
    "sitting on a swing bench on a porch in golden hour",
    "holding a kitten up to the camera in a living room",
    "trying on a hat at a street market stall",
    "eating ice cream on a seaside promenade, windy",
    "crouching to photograph flowers in a garden",
    "doing a yoga pose on a rooftop at sunrise",
    "waving from a bicycle on a river path",
    "reading a menu outside a restaurant at dusk",
    "catching a soap bubble in a backyard",
    "clinking a coffee cup toward the camera in a cafe",
    "standing at a lookout with mountains behind, wind in the hair",
    "sitting on museum steps eating a sandwich",
    "walking a golden retriever in a park",
    "pushing a shopping cart in a bright supermarket aisle",
    "under an umbrella in light rain by a canal",
    "feeding pigeons in an old town square",
    "stretching after a run on an athletics track",
    "holding a balloon bunch at a fairground",
]

PLACES = [
    "alpine lake with mirror-still water reflecting snow-capped peaks and pine trees",
    "wide sandy beach with gentle waves and a distant pier",
    "flat snow-covered field with a single bare tree and a wooden fence",
    "foggy pine forest with a narrow trail disappearing into the mist",
    "city skyline across a river with glass skyscrapers",
    "sand dunes with crisp ridge lines and a lone hiker far away",
    "green rice terraces on a hillside with water in the paddies",
    "tall waterfall in a mossy gorge with silky water",
    "grey sea with a distant lighthouse and a rocky shore",
    "straight country road through an autumn forest with red and yellow leaves",
    "path lined with hundreds of vermilion torii gates, no people",
    "mountain ridge under a star-filled sky with the Milky Way",
    "old Chinese water town with stone bridges and canals",
    "the Bund waterfront in Shanghai with historic buildings",
    "narrow hutong alley with grey brick walls and bicycles",
    "modern glass office tower reflecting clouds, looking up",
    "a busy pedestrian shopping street with shop signs and crowds",
    "a quiet suburban street with parked cars and trees",
    "the Great Wall winding over green mountains",
    "a tropical island beach with palm trees and turquoise water",
    "terraced vineyard hills in soft evening light",
    "a highway interchange from above with light trails",
    "a lavender field with a stone farmhouse",
    "a canyon with layered red rock walls",
    "a harbour full of sailboats with a tilted horizon of about 8 degrees",
    "a Japanese zen garden with raked gravel and a maple tree",
    "a European old town square with a cathedral and cafes",
    "a lake with a wooden pier and mountains in morning mist",
    "a lotus pond with blooming pink flowers and lily pads",
    "a bamboo forest path with tall green stalks",
    "a desert highway stretching to the horizon",
    "a railway station platform with a train arriving",
    "a university campus lawn with old brick buildings",
    "a night street with rain-wet asphalt reflecting neon",
    "a cliff coastline with waves crashing on rocks",
    "a volcano with a lake in its crater",
    "a cherry blossom avenue in full bloom",
    "a small fishing village with colourful boats",
    "a rooftop view of a dense old city with tiled roofs",
    "a snowy mountain village with lit windows at dusk",
]
PLACE_CONDITIONS = [("sunrise", "at sunrise with pink and orange clouds"), ("noon", "at midday under a clear blue sky"), ("overcast", "on an overcast day with flat soft light")]
THINGS = [
    "a bowl of steaming beef noodle soup on a wooden table", "a plate of dim sum in bamboo steamers",
    "a latte with foam art next to a croissant", "a birthday cake with strawberries on a white plate",
    "a sushi platter on a dark slate", "a slice of pizza being lifted with cheese strings",
    "a tabby cat sleeping on a windowsill", "a golden retriever puppy on a lawn",
    "a parrot on a perch with colourful feathers", "a goldfish in a round glass bowl",
    "a mechanical wristwatch on a leather strap, macro", "a pair of white sneakers on a concrete floor",
    "a vintage film camera on a wooden desk", "a bouquet of tulips in a glass vase",
    "a red sports car parked by the sea", "a bicycle leaning against a brick wall",
    "a laptop and notebook on a cafe table from above", "a stack of old books with a pair of glasses",
    "a printed restaurant receipt on a table", "a whiteboard covered in handwritten diagrams",
    "a smartphone screen showing a map app, held in a hand", "a computer monitor showing a spreadsheet, photographed at an angle",
    "a printed document with a signature and a stamp, flat lay", "a business card on a marble surface",
    "a bowl of fresh fruit: apples, grapes and bananas", "a wooden chopping board with sliced vegetables",
    "a ceramic teapot and two cups on a tray", "a glass of red wine on a windowsill at sunset",
    "a hiking backpack and boots by a tent", "a guitar on a bed in a sunny bedroom",
    "a child's drawing taped to a fridge", "a houseplant in a terracotta pot by a window",
    "a classic motorcycle in a garage", "a toy train set on a carpet",
    "a wedding ring in an open velvet box", "a pair of chopsticks on a bowl of rice",
    "a window display of a bakery with bread loaves", "a street food cart with skewers grilling",
    "a cat looking directly into the camera, close-up", "a dog with its head out of a car window",
    "a museum label next to a painting, close-up", "a road sign at a rural junction",
    "a bento box with rice, egg and vegetables", "a hotpot table with raw ingredients",
    "a pile of autumn leaves on a wet pavement", "a snowman with a scarf in a garden",
    "a stack of moving boxes in an empty room", "an airplane window view of clouds and a wing",
    "a receipt-sized QR code sticker on a shop counter", "a handwritten shopping list on a notepad",
]
DEFECT_SCENES = [
    ("red-coat", "a woman in a red coat standing in a park"), ("sofa", "a man sitting on a sofa in a living room"),
    ("kids-beach", "children playing on a beach"), ("bar", "friends at a dim bar"),
    ("harbour", "a harbour with sailboats"), ("portrait-sun", "a man outdoors with the sun behind him"),
    ("street-cross", "people crossing a busy street"), ("cake", "a birthday cake on a table with people around"),
    ("garden", "an old couple on a garden bench"), ("temple", "tourists in front of a temple gate"),
    ("dog-park", "a dog running in a park"), ("city-night", "a city street at night"),
]
DEFECT_KINDS = [
    ("blur", "the entire image badly out of focus, everything soft and blurry", ["blur", "severe"]),
    ("under", "severely underexposed, almost black with only faint details", ["underexposed", "severe"]),
    ("over", "severely overexposed and washed out, bright areas pure white", ["overexposed", "severe"]),
    ("noise", "extremely high ISO with heavy colour noise and grain, muddy shadows", ["noisy", "severe"]),
    ("shake", "smeared by camera shake, double edges everywhere", ["blur", "shake"]),
]


# ------------------------------------------------------------------------------------- graphs
def common(prompt: str, seed: int, steps: int, prefix: str, denoise: float = 1.0) -> dict:
    return {
        "1": {"class_type": "UnetLoaderGGUF", "inputs": {"unet_name": UNET}},
        "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": CLIP, "type": "qwen_image", "device": "default"}},
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": VAE}},
        "6": {
            "class_type": "KSampler",
            "inputs": {
                "model": ["1", 0], "positive": ["4", 0], "negative": ["4", 1], "latent_image": ["5", 0],
                "seed": seed, "steps": steps, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple", "denoise": denoise,
            },
        },
        "7": {"class_type": "VAEDecode", "inputs": {"samples": ["6", 0], "vae": ["3", 0]}},
        "8": {"class_type": "SaveImage", "inputs": {"images": ["7", 0], "filename_prefix": prefix}},
    }


def graph_t2i(prompt, seed, w, h, prefix, steps):
    g = common(prompt, seed, steps, prefix)
    g["4"] = {"class_type": "TextEncodeQwenImage21", "inputs": {"clip": ["2", 0], "prompt": prompt, "negative_prompt": "", "resolution": 1024, "vae": ["3", 0]}}
    g["5"] = {"class_type": "EmptyLatentImage", "inputs": {"width": w, "height": h, "batch_size": 1}}
    return g


def graph_ref(prompt, seed, w, h, prefix, steps, refs: list[str]):
    """Visual reference mode (vae not connected to the encoder, as in the saved Reference workflow);
    the output size comes from an EmptyLatentImage so the aspect ratio is ours."""
    g = common(prompt, seed, steps, prefix)
    enc = {"clip": ["2", 0], "prompt": prompt, "negative_prompt": "", "resolution": 768}
    for i, path in enumerate(refs, 1):
        g[f"1{i}"] = {"class_type": "LoadImage", "inputs": {"image": path}}
        enc[f"images.image_{i}"] = [f"1{i}", 0]
    g["4"] = {"class_type": "TextEncodeQwenImage21", "inputs": enc}
    g["5"] = {"class_type": "EmptyLatentImage", "inputs": {"width": w, "height": h, "batch_size": 1}}
    return g


def graph_i2i(prompt, seed, prefix, steps, base: str, denoise: float):
    g = common(prompt, seed, steps, prefix, denoise)
    g["4"] = {"class_type": "TextEncodeQwenImage21", "inputs": {"clip": ["2", 0], "prompt": prompt, "negative_prompt": "", "resolution": 1024, "vae": ["3", 0]}}
    g["10"] = {"class_type": "LoadImage", "inputs": {"image": base}}
    g["5"] = {"class_type": "VAEEncode", "inputs": {"pixels": ["10", 0], "vae": ["3", 0]}}
    return g


# --------------------------------------------------------------------------------------- jobs
def actor_ref(a: str) -> str:
    return f"{OUT}/actors/{a}_00001_.png [output]"


def build_jobs(rng: random.Random) -> list[dict]:
    jobs: list[dict] = []

    def add(**j):
        j.setdefault("tags", [])
        j.setdefault("size", "3:2")
        jobs.append(j)

    # actors: clean front-facing references
    for i, (a, desc) in enumerate(ACTORS.items()):
        add(phase="actors", kind="t2i", name=a, scene=f"actor-{a}", seed=900000 + i, size="1:1",
            prompt=f"{STYLE} Head-and-shoulders portrait of {desc}, looking straight at the camera with a neutral friendly expression, wearing {ACTOR_WARDROBE[a][0]}, plain light grey studio background, soft even light, sharp focus on the eyes.",
            tags=["actor", a])

    # people: every actor in every single scene (wardrobe rotates)
    for si, (sid, scene, size) in enumerate(SINGLE_SCENES):
        for ai, a in enumerate(ACTORS):
            add(phase="people", kind="ref", name=f"{sid}-{a}", scene=f"people-{sid}", seed=100000 + si * 10 + ai, size=size, refs=[a], actors=[a],
                prompt=f"{STYLE} <image1> is the same person: keep this exact face and identity. {ACTORS[a].capitalize()} wearing {ACTOR_WARDROBE[a][(si + ai) % 4]}, {scene}.",
                tags=["single", a, sid])

    # groups: 30 scenes x 4 takes (seed + wardrobe vary, same actors)
    for gi, (gid, actors, scene, size) in enumerate(GROUP_SCENES):
        for take in range(4):
            who = ", ".join(f"<image{i + 1}> is {ACTORS[a]} wearing {ACTOR_WARDROBE[a][(take + i) % 4]}" for i, a in enumerate(actors))
            add(phase="groups", kind="ref", name=f"{gid}-{take + 1}", scene=f"group-{gid}", seed=200000 + gi * 10 + take, size=size, refs=list(actors), actors=list(actors),
                prompt=f"{STYLE} Group photo of {len(actors)} people; keep each referenced face and identity exactly: {who}. They are {scene}.",
                tags=["group", str(len(actors)), gid, *actors])

    # bursts: 30 single-actor action bases + 30 group bases, then derived frames
    actor_cycle = list(ACTORS)
    for bi, action in enumerate(BURST_SINGLE_ACTIONS):
        a = actor_cycle[bi % len(actor_cycle)]
        base = f"bs{bi + 1:02d}-{a}"
        bprompt = f"{STYLE} <image1> is the same person: keep this exact face and identity. {ACTORS[a].capitalize()} wearing {ACTOR_WARDROBE[a][bi % 4]}, {action}."
        add(phase="bursts", kind="ref", name=f"{base}-00", scene=f"burst-{base}", burst=base, seed=300000 + bi, refs=[a], actors=[a],
            prompt=f"{bprompt} Eyes open, natural expression, sharp.", tags=["burst", "base", a])
        frames = rng.sample(BURST_FRAMES, k=rng.randint(4, 6))
        for fi, (delta, tags) in enumerate(frames, 1):
            add(phase="bursts", kind="i2i", name=f"{base}-{fi:02d}", scene=f"burst-{base}", burst=base, seed=300000 + bi * 10 + fi, actors=[a],
                base=f"{OUT}/bursts/{base}-00_00001_.png [output]", denoise=0.45,
                prompt=f"{bprompt} {delta}", tags=["burst", a, *tags])
    for bi, (gid, actors, scene, size) in enumerate(GROUP_SCENES):
        base = f"bg{bi + 1:02d}-{gid}"
        who = ", ".join(f"<image{i + 1}> is {ACTORS[a]}" for i, a in enumerate(actors))
        bprompt = f"{STYLE} Group photo of {len(actors)} people; keep each referenced face and identity exactly: {who}. They are {scene}."
        add(phase="bursts", kind="ref", name=f"{base}-00", scene=f"burst-{base}", burst=base, seed=310000 + bi, size=size, refs=list(actors), actors=list(actors),
            prompt=f"{bprompt} Everyone with eyes open looking at the camera, sharp.", tags=["burst", "base", "group", *actors])
        n = rng.randint(4, 6)
        for fi in range(1, n + 1):
            victim = rng.choice(actors)
            delta, tags = rng.choice(BURST_FRAMES[1:])
            add(phase="bursts", kind="i2i", name=f"{base}-{fi:02d}", scene=f"burst-{base}", burst=base, seed=310000 + bi * 10 + fi, actors=list(actors),
                base=f"{OUT}/bursts/{base}-00_00001_.png [output]", denoise=0.45,
                prompt=f"{bprompt} Everyone as before except the person who is {ACTORS[victim]}: {delta}.", tags=["burst", "group", f"victim:{victim}", *tags])

    # places
    for pi, place in enumerate(PLACES):
        for ci, (cid, cond) in enumerate(PLACE_CONDITIONS):
            size = "16:9" if pi % 5 == 0 else ("2:3" if pi % 7 == 0 else "3:2")
            add(phase="places", kind="t2i", name=f"pl{pi + 1:02d}-{cid}", scene=f"place-{pi + 1:02d}", seed=400000 + pi * 10 + ci, size=size,
                prompt=f"{STYLE} Landscape photograph of {place}, {cond}, no people in the frame.", tags=["place", cid, *(["tilted"] if "tilted" in place else [])])

    # things
    for ti, thing in enumerate(THINGS):
        for v in range(2):
            size = "1:1" if ti % 3 == 0 else ("4:3" if v else "3:2")
            light = "natural window light" if v == 0 else "warm indoor lighting"
            add(phase="things", kind="t2i", name=f"th{ti + 1:02d}-{v + 1}", scene=f"thing-{ti + 1:02d}", seed=500000 + ti * 10 + v, size=size,
                prompt=f"{STYLE} Photograph of {thing}, {light}, no people.", tags=["thing", *(["document"] if any(k in thing for k in ("receipt", "document", "screen", "monitor", "whiteboard", "card", "list", "QR", "label")) else [])])

    # defects
    for di, (did, scene) in enumerate(DEFECT_SCENES):
        for ki, (kid, mod, tags) in enumerate(DEFECT_KINDS):
            add(phase="defects", kind="t2i", name=f"df{di + 1:02d}-{kid}", scene=f"defect-{di + 1:02d}", seed=600000 + di * 10 + ki,
                prompt=f"{STYLE} {scene.capitalize()}; the photo is {mod}.", tags=["defect", *tags])
    return jobs


# ------------------------------------------------------------------------------------- driver
def api(server, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(server + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def wait_done(server, prompt_id, timeout_s=1200):
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        hist = api(server, f"/history/{prompt_id}")
        if prompt_id in hist:
            st = hist[prompt_id].get("status", {})
            if st.get("status_str") == "error":
                raise RuntimeError(json.dumps(st.get("messages"))[:1500])
            if st.get("completed", True):
                return hist[prompt_id]
        time.sleep(1.5)
    raise TimeoutError(prompt_id)


def job_graph(j, steps):
    prefix = f"{OUT}/{j['phase']}/{j['name']}"
    w, h = SIZES[j["size"]]
    if j["kind"] == "t2i":
        return graph_t2i(j["prompt"], j["seed"], w, h, prefix, steps)
    if j["kind"] == "ref":
        return graph_ref(j["prompt"], j["seed"], w, h, prefix, steps, [actor_ref(a) for a in j["refs"]])
    return graph_i2i(j["prompt"], j["seed"], prefix, steps, j["base"], j["denoise"])


def save_workflows(comfy: Path):
    d = comfy / "user/default/workflows/imagepicker-testset"
    d.mkdir(parents=True, exist_ok=True)
    ex = {
        "t2i-text-to-image.api.json": graph_t2i(f"{STYLE} A woman reading by a window.", 1, 1536, 1024, "imagepicker-testset/example", 25),
        "ref-identity-reference.api.json": graph_ref(f"{STYLE} <image1> is the same person: keep this exact face. She is walking in a park.", 1, 1536, 1024, "imagepicker-testset/example", 25, [actor_ref("a1")]),
        "i2i-burst-frame.api.json": graph_i2i(f"{STYLE} Same photo, both eyes closed mid-blink.", 1, "imagepicker-testset/example", 25, f"{OUT}/bursts/bs01-a1-00_00001_.png [output]", 0.45),
    }
    for name, g in ex.items():
        (d / name).write_text(json.dumps(g, indent=1, ensure_ascii=False))
    (d / "README.md").write_text(
        "imagePicker evaluation-set workflows (API format; drag into the ComfyUI UI to load).\n"
        "Driven by scripts/comfy-testset-v2.py in the imagePicker repo.\n"
        "- t2i: plain text to image (Qwen-Image 2.1 GGUF Q8, 25 steps, CFG 1, euler/simple)\n"
        "- ref: identity reference via TextEncodeQwenImage21 images.image_N (VAE left unconnected = visual reference), size from EmptyLatentImage\n"
        "- i2i: burst frame derivation, VAEEncode of the base frame + KSampler denoise 0.45\n"
    )
    print(f"workflows written to {d}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8188")
    ap.add_argument("--comfy", default=str(Path.home() / "ai/ComfyUI"))
    ap.add_argument("--only", default="")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--steps", type=int, default=25)
    ap.add_argument("--denoise", type=float, default=None, help="override burst-frame denoise")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--save-workflows", action="store_true")
    ap.add_argument("--pilot", action="store_true", help="actors a1 only, one people scene, one burst base + 3 frames")
    args = ap.parse_args()
    comfy = Path(args.comfy)
    if args.save_workflows:
        save_workflows(comfy)
        return 0
    jobs = build_jobs(random.Random(7))
    if args.only:
        keep = set(args.only.split(","))
        jobs = [j for j in jobs if j["phase"] in keep]
    if args.pilot:
        jobs = [j for j in jobs if j["name"] in ("a1", "park-golden-a1", "bs01-a1-00", "bs01-a1-01", "bs01-a1-02", "bs01-a1-03")]
    if args.denoise is not None:
        for j in jobs:
            if j["kind"] == "i2i":
                j["denoise"] = args.denoise
    if args.limit:
        jobs = jobs[: args.limit]
    kinds = {k: sum(1 for j in jobs if j["kind"] == k) for k in ("t2i", "ref", "i2i")}
    print(f"{len(jobs)} images {kinds}", flush=True)
    if args.dry_run:
        for j in jobs[:: max(1, len(jobs) // 40)]:
            print(f"{j['phase']}/{j['name']:28s} {j['kind']} seed={j['seed']} {j['size']} {j['prompt'][:70]}…")
        return 0
    try:
        api(args.server, "/queue")
    except (urllib.error.URLError, OSError) as e:
        print(f"ComfyUI not reachable: {e}", file=sys.stderr)
        return 1
    out_root = comfy / "output" / OUT
    out_root.mkdir(parents=True, exist_ok=True)
    mpath = out_root / "manifest.json"
    manifest = json.loads(mpath.read_text()) if mpath.exists() else {"images": []}
    done = {m["name"] for m in manifest["images"]}
    client = uuid.uuid4().hex
    t0 = time.monotonic()
    failures = 0
    for n, j in enumerate(jobs, 1):
        if j["name"] in done:
            continue
        t = time.monotonic()
        try:
            resp = api(args.server, "/prompt", {"prompt": job_graph(j, args.steps), "client_id": client})
            entry = wait_done(args.server, resp["prompt_id"])
        except Exception as e:  # noqa: BLE001
            failures += 1
            print(f"[{n}/{len(jobs)}] FAILED {j['phase']}/{j['name']}: {str(e)[:300]}", flush=True)
            if failures > 20:
                print("too many failures, stopping", flush=True)
                return 1
            continue
        files = [f"{o['subfolder']}/{o['filename']}" if o.get("subfolder") else o["filename"] for node in entry.get("outputs", {}).values() for o in node.get("images", [])]
        w, h = SIZES[j["size"]]
        rec = {k: v for k, v in j.items() if k not in ("refs", "base")}
        rec.update({"category": j["phase"], "width": w, "height": h, "steps": args.steps, "files": files})
        manifest["images"].append(rec)
        mpath.write_text(json.dumps(manifest, ensure_ascii=False, indent=1))
        done.add(j["name"])
        print(f"[{n}/{len(jobs)}] {j['phase']}/{j['name']} {j['kind']} {time.monotonic() - t:.0f}s", flush=True)
    print(f"finished in {(time.monotonic() - t0) / 60:.1f} min, {failures} failures; manifest {mpath}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
