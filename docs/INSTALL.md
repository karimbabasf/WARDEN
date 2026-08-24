# Installing WARDEN

WARDEN ships as a single disk image (a `.dmg` file). Here's what to expect the
first time you open it.

## 1. Download and install

Open the `.dmg` from your downloads, then drag **WARDEN** into your
**Applications** folder, the same as any other Mac app.

## 2. The first-open warning

The first time you open WARDEN, macOS will likely say:

> "WARDEN" is damaged and can't be opened. You should move it to the Trash.

**WARDEN is not damaged.** This is the standard warning macOS shows for any
app downloaded from outside the App Store that Apple hasn't notarized yet.
It's a caution label, not a diagnosis. Here's how to get past it:

**Try this first:** in Finder, right-click (or Control-click) WARDEN in your
Applications folder and choose **Open** from the menu, then click **Open**
again in the dialog that appears.

**If that doesn't work:** open **System Settings**, go to **Privacy &
Security**, scroll down to the **Security** section, and click **Open
Anyway** next to the message about WARDEN. Confirm once more and it will
launch. This button only appears for about an hour after you've tried to
open WARDEN at least once, so if you don't see it, try opening WARDEN again
first.

**If neither works:** open **Terminal** and run:

```
xattr -cr /Applications/WARDEN.app
```

then open WARDEN normally. This clears the "downloaded from the internet"
flag that's causing the warning.

None of this will be necessary once WARDEN is signed and notarized by Apple.
That's coming; this is the honest state of things until it does.

## 3. Apple Silicon only, for now

WARDEN currently runs on Apple Silicon Macs only: M1, M2, M3, M4, or newer.
It does not yet run on Intel Macs. To check which one you have, click the
Apple menu, choose **About This Mac**, and look at the chip listed. If it
starts with "Apple," you're set.

## 4. No key, no account

WARDEN is MIT licensed and free. It opens straight to the radar: there is no
license key, no activation screen, and no sign-in. It makes no network calls
and reads only the agent transcripts already on your disk.

You can also skip the `.dmg` and build it yourself. See the Quickstart in
[`../README.md`](../README.md).
