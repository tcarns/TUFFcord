"""Voice end to end with the app's real voice code: three bots in a channel each
play a tone; each hears the others' tones and not its own. Muting halfway
stops a voice; a deafened bot hears nothing and sends nothing; someone in
another channel isn't heard."""
import json, os, subprocess

from lib import Server, binary, check, main, spawn

TONES = {"A": 440, "B": 660, "C": 880}


async def run():
    s = Server()
    procs = {}

    def start(name, *args, channel="Lounge"):
        out = os.path.join(s.dir, f"{name}.out")
        procs[name] = (spawn([binary("tuffcord-bot"), "--server", s.url, "--password", s.invite(), "--name", name,
                              "--channel", channel, "--seconds", "8", *args],
                             stdout=open(out, "w"), stderr=subprocess.STDOUT), out)

    listen = ",".join(str(t) for t in TONES.values()) + ",1000"
    start("BotA", "--tone", "440", "--listen", listen)
    start("BotB", "--tone", "660", "--listen", listen, "--mute-after", "3")
    start("BotC", "--tone", "880", "--listen", listen, "--deafen")
    start("BotD", "--tone", "1000", "--listen", listen, channel="Gaming")
    r = {}
    for name, (p, out) in procs.items():
        p.wait(60)
        try:
            r[name] = json.load(open(out))
        except ValueError:
            r[name] = {}
    # Missing measurements count as loud, so a bot that didn't run can't pass a "quiet" check.
    heard = lambda who, half, tone: r[who].get(f"heard{half}HalfDb", {}).get(str(tone), 0)
    loud, quiet = -40, -60
    check(all(x.get("ok") for x in r.values()), "the bots ran without errors")
    check(heard("BotA", "First", 660) > loud, "A hears B")
    check(heard("BotA", "First", 440) < quiet, "A doesn't hear itself")
    check(heard("BotA", "First", 1000) < quiet, "A doesn't hear D in another channel")
    check(heard("BotA", "Second", 660) < quiet, "after B mutes, A stops hearing B")
    check(heard("BotC", "First", 440) < quiet and heard("BotC", "First", 660) < quiet, "deafened C hears nothing")
    check(heard("BotA", "First", 880) < quiet and r["BotC"].get("framesSent", 1) == 0,
          "deafened C is muted too (sends nothing)")
    check(heard("BotB", "First", 440) > loud, "B hears A")
    # A frame already on its way when the mute lands is fine.
    check(r["BotB"].get("framesSentWhileMuted", 99) <= 2, "nothing more is sent once muted")


main(run)
