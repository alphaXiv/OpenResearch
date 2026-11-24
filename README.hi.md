<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="56" align="absmiddle" /> OpenResearch</h1>

रिसर्च एजेंटों के लिए लोकल-फ़र्स्ट हार्नेस और वर्कस्पेस।

<p><em>an <a href="https://alphaxiv.org"><img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv</a> project</em></p>

<p>
<a href="https://github.com/alphaXiv/OpenResearch/releases/latest"><img src="https://img.shields.io/github/v/release/alphaXiv/OpenResearch?style=flat-square" alt="Latest release" /></a>
<a href="https://github.com/alphaXiv/OpenResearch/blob/main/LICENSE"><img src="https://img.shields.io/github/license/alphaXiv/OpenResearch?style=flat-square" alt="License" /></a>
</p>

<p><a href="README.md">English</a> · <a href="README.es.md">Español</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">简体中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ru.md">Русский</a> · <a href="README.tr.md">Türkçe</a> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.te.md">తెలుగు</a></p>

<p><img src=".github/readme-assets/openresearch-demo.gif" alt="OpenResearch डेस्कटॉप ऐप: एक एजेंट ट्रेनिंग रन की समस्या पता लगाने के लिए alphaXiv पर खोज करता है, फिर उसके Trackio चार्ट और नतीजों के फ़िगर खोलता है" width="680" /></p>

<hr />

<p>चाहे आपको कोपायलट चाहिए या ऑटोरिसर्च टूल, OpenResearch आपके काम की रफ़्तार बढ़ाएगा। ऐसे रिसर्च एजेंट लॉन्च करें जो साहित्य की समीक्षा कर सकें, परिकल्पनाएँ विकसित कर सकें, प्रयोग चला सकें और रिसर्च आर्टिफ़ैक्ट तैयार कर सकें।</p>

<p>
<a href="https://openresearch.sh/dl/macos?src=github"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-macos-dark.svg"><img src=".github/readme-assets/download-macos.svg" alt="macOS के लिए OpenResearch डाउनलोड करें" width="220" height="44" /></picture></a>
<a href="https://openresearch.sh/dl/windows?src=github"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Windows (बीटा) के लिए OpenResearch डाउनलोड करें" width="220" height="44" /></picture></a>
<a href="https://openresearch.sh/dl/linux_x64?src=github"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-linux-dark.svg"><img src=".github/readme-assets/download-linux.svg" alt="Linux के लिए OpenResearch डाउनलोड करें" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="दस्तावेज़" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/alphaXiv/OpenResearch/releases"><img src=".github/readme-assets/action-releases.svg" alt="रिलीज़" width="78" height="24" /></a>
</p>

<p><sub>macOS 11+ · Windows बीटा के लिए <a href="https://gitforwindows.org">Git for Windows</a> ज़रूरी है · <a href="https://openresearch.sh/docs/install">Linux</a> ऐप के लिए glibc 2.35+ चाहिए</sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

## शुरू करें

**हम OpenResearch डेस्कटॉप ऐप इस्तेमाल करने की सलाह देते हैं।** अपने प्लेटफ़ॉर्म के लिए इसे डाउनलोड करें:

- [macOS](https://openresearch.sh/dl/macos?src=github)
- [Windows (बीटा)](https://openresearch.sh/dl/windows?src=github)
- Linux: [x86_64](https://openresearch.sh/dl/linux_x64?src=github) · [ARM64](https://openresearch.sh/dl/linux_arm64?src=github)

<details>
<summary>स्टैंडअलोन CLI पसंद है? macOS या Linux पर इंस्टॉल करें</summary>

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` लोकल डैशबोर्ड को `http://127.0.0.1:4791` पर खोलता है।

किसी संस्था द्वारा मैनेज किए जाने वाले Mac पर, डिवाइस-मैनेजमेंट नीतियाँ `install.sh` से
इंस्टॉल किए गए CLI को ब्लॉक कर सकती हैं, क्योंकि वह अभी साइन नहीं किया गया है। इसके बजाय
साइन और नोटराइज़ किया गया डेस्कटॉप ऐप इस्तेमाल करें। आप ऐप की
सेटिंग्स → अपडेट → **`orx` कमांड इंस्टॉल करें** से अपने टर्मिनल के लिए `orx` भी इंस्टॉल कर सकते हैं।

</details>

ईमेल पर अपडेट पाने और मैनेज्ड OpenResearch कंप्यूट इस्तेमाल करने के लिए
[openresearch.sh](https://openresearch.sh) पर खाता बनाएँ।

## यह कैसे काम करता है

**अपना पसंदीदा कोडिंग एजेंट इस्तेमाल करें।** OpenResearch <img src=".github/readme-assets/claude.svg" alt="" width="18" height="18" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="18" height="18" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="18" height="18" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="18" height="18" align="texttop" /> Cursor और
<img src=".github/readme-assets/antigravity.svg" alt="" width="18" height="18" align="texttop" /> Google Antigravity के साथ सीधे काम करता है।
आप OpenCode के साथ
<img src=".github/readme-assets/lmstudio.svg" alt="" width="18" height="18" align="texttop" /> LM Studio,
<img src=".github/readme-assets/omlx.svg" alt="" width="18" height="18" align="texttop" /> oMLX,
<img src=".github/readme-assets/rapid-mlx.svg" alt="" width="18" height="18" align="texttop" /> Rapid-MLX,
<img src=".github/readme-assets/ollama.svg" alt="" width="18" height="18" align="texttop" /> Ollama या किसी कस्टम एंडपॉइंट के ज़रिए [लोकल मॉडल भी इस्तेमाल कर सकते हैं](https://openresearch.sh/docs/agents)।

**रिसर्च परिकल्पनाओं को नवीनतम साहित्य पर आधारित करें।** OpenResearch <img src=".github/readme-assets/alphaxiv.svg" alt="" width="18" height="18" align="absmiddle" /> alphaXiv, <img src=".github/readme-assets/biorxiv.svg" alt="" width="18" height="18" align="absmiddle" /> bioRxiv और <img src=".github/readme-assets/pubmed.svg" alt="" width="18" height="18" align="absmiddle" /> PubMed के साथ इंटीग्रेट होता है, ताकि एजेंट प्रासंगिक पेपर ढूँढ सकें, सोच-समझकर परिकल्पनाएँ बना सकें और अपने प्रयोगों को मौजूदा रिसर्च से जोड़ सकें।

**अपना कंप्यूट चुनें।** प्रयोग लोकल रूप से चलाएँ या SSH,
Slurm,
<img src=".github/readme-assets/kubernetes.svg" alt="" width="18" height="18" align="texttop" /> Kubernetes,
<img src=".github/readme-assets/modal.svg" alt="" width="18" height="18" align="texttop" /> Modal,
<img src=".github/readme-assets/thinking-machines.svg" alt="" width="18" height="18" align="texttop" /> Tinker,
<img src=".github/readme-assets/ray.svg" alt="" width="18" height="18" align="texttop" /> Ray,
<img src=".github/readme-assets/huggingface.svg" alt="" width="18" height="18" align="texttop" /> Hugging Face Jobs और
मैनेज्ड OpenResearch कंप्यूट का इस्तेमाल करें।

**एजेंटों को हर प्रयोग की याद दें।** OpenResearch हर प्रयोग को एक लोकल SQL डेटाबेस में दर्ज करता है और उसके लॉग, कोड
और आर्टिफ़ैक्ट आपकी मशीन पर रखता है, ताकि एजेंट
पिछले नतीजों से सीख सकें और तय कर सकें कि आगे क्या आज़माना है। डेस्कटॉप ऐप से यह
देखना आसान हो जाता है कि आपके एजेंटों ने क्या किया है, और हर नतीजे के पीछे के सबूत जाँचना भी।

**अपनी रिसर्च अपने पास और अपने अधिकार में रखें।** सब कुछ पूरी तरह लोकल रूप से चलाएँ। हम आपका कोड या एजेंट ट्रेस इकट्ठा नहीं करते। आपके प्रोजेक्ट,
बातचीत, प्रयोग, लॉग और आर्टिफ़ैक्ट आपकी मशीन पर, आपके
नियंत्रण में रहते हैं।

## उपयोग एनालिटिक्स

आधिकारिक रिलीज़ बिल्ड बैकग्राउंड में मोटे तौर के प्रोडक्ट एनालिटिक्स इवेंट और एजेंट का विस्तृत फ़ीडबैक भेजते हैं, जो एक रैंडम
इंस्टॉलेशन ID से जुड़े होते हैं। ये डिफ़ॉल्ट रूप से चालू रहते हैं और आप इन्हें बंद कर सकते हैं। आपका कोड, प्रॉम्प्ट, फ़ाइलों की सामग्री या पाथ,
रिपॉज़िटरी के नाम, टोकन, ईमेल, या प्रोजेक्ट और प्रयोग के आइडेंटिफ़ायर इकट्ठा नहीं किए जाते। गुमनाम टोकन/संदेश गिनती से यह पता लगाया जाता है
कि OpenResearch में कौन-से मॉडल इस्तेमाल हो रहे हैं, और इसे https://openresearch.sh/rankings पर देखा जा सकता है। एजेंट की फ़ीडबैक रिपोर्ट
बग रिपोर्ट, फ़ीचर अनुरोधों और OpenResearch से जुड़ी आम परेशानियों के लिए भेजी जाती हैं। एजेंटों को
निर्देश दिया गया है कि वे रिपोर्ट से आपकी रिसर्च या कोड के बारे में हर संवेदनशील जानकारी हटा दें।

आप डेस्कटॉप ऐप की
सेटिंग्स में या `orx telemetry off` से सभी उपयोग एनालिटिक्स और एजेंट फ़ीडबैक बंद कर सकते हैं।
