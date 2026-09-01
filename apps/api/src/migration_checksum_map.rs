#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoricalMigrationChecksum {
    pub(crate) version: i64,
    pub(crate) previous_sha384: &'static str,
    pub(crate) current_sha384: &'static str,
}

pub(crate) const HISTORICAL_MIGRATION_CHECKSUMS: &[HistoricalMigrationChecksum] = &[
    HistoricalMigrationChecksum {
        version: 1,
        previous_sha384: "6fc3df625d51d9f6e8af9e2f79f9ccbb34151bad6666f735231af878bb24baca492a68c3e75d2f461a60993af81e5178",
        current_sha384: "98e3bf4cd6dd83d2ad70cf559e3709111e48e143c5fae9dee76975e63df8d429e2c3bbe9f7511c2a3c4056913497f76a",
    },
    HistoricalMigrationChecksum {
        version: 2,
        previous_sha384: "5082f2f5e0c6de2c17ec3a59526f3406bbb2854610d22c04f335c29449aff4a80dbf30b0a984a18368418c0c464282a5",
        current_sha384: "c6d5e049a0e0625ea894976ef539b4971100dac2ff5d68014a2566831980e1d7b68ad71712657b59dc1a09bb9248b8a1",
    },
    HistoricalMigrationChecksum {
        version: 3,
        previous_sha384: "f1c5707d7300923a1429101d3bade7e6c8aaad940c2af4b4c31403524675df42bacdd2e53ff6bb3fc058d55ae87de930",
        current_sha384: "f27e38553b2d3e410341a2eac88cd7860d1b0ee4426d507b871d2865f017627d007d0bf5f6548ad8673d60b3bdde9e15",
    },
    HistoricalMigrationChecksum {
        version: 4,
        previous_sha384: "fd141e7266adf031fc1de897d6a541d43147edbd49860181aa8951af9e9ec7d7778a8dfe1cef4df433878bd2be8e82ff",
        current_sha384: "56243aa5b6568fbf80bea9fa0a2e4fedb84f4cb83c9fb08edf3c6afce77c5848f492dd17d56ab4fdaa1c7784934b8300",
    },
    HistoricalMigrationChecksum {
        version: 5,
        previous_sha384: "701e96bff6505a7a309f92ef80173067897c2431e2f0ba6396cccc31a5c0eea7890d6a86273bc83b2dbe217fae8de79c",
        current_sha384: "015f76da12703274a5c6387f57269534834061a7df6eab6ffa0cba48f2ea5e8707f531f88ce77c4632c53f8dbc1f8e77",
    },
    HistoricalMigrationChecksum {
        version: 6,
        previous_sha384: "db93669d33dcfdd6537a68baaf89558008030e41637e8904bb635ce90cfd4c0952cdbe840f9702f6ebdcac59ee48ef80",
        current_sha384: "487e437d50ae7715ffd950a2ef498ba594dbc0b4cce6bff4b8efa67fcdc483b2fc9a32c89c808fd8edc7dc7ab06036c2",
    },
    HistoricalMigrationChecksum {
        version: 7,
        previous_sha384: "5e1ed63ed21e0ec9973616b7e20ba285509031e0be7814b5675535af24084e0fbb3d64e975a5e63595b960dc86152bbc",
        current_sha384: "fab9130a81bb6f46d6e0e48409aea507c380345054b55593bd781c04a24308caa96b18a85916b52173b830666d5e5b76",
    },
    HistoricalMigrationChecksum {
        version: 8,
        previous_sha384: "1ac882a7fa301ff0154de79acdbc8245453d67ef2963fcd74755b26667114ccf2d88ca123a3799a414f4d8ae674b9321",
        current_sha384: "6763306a5a8b9619429e699a111593e1a5fdcbcf1db2a986fa26c9b4db473a80b5dc4c5d655b18e8688de3a0467cb2b2",
    },
    HistoricalMigrationChecksum {
        version: 9,
        previous_sha384: "e20987fc94ba2f0939898026dc6c26994b908f0b46a91b8b4d0ae679cda66f54e9134c2d19bd92e75eae856faea7d705",
        current_sha384: "93405c44528dda2e7191398d3788500396261db703a7e5d490d396b7e0b90c699b8bdaeac036873ca1e007b8544b9c40",
    },
    HistoricalMigrationChecksum {
        version: 10,
        previous_sha384: "55614df145eb4d97c59946a739e5d7a0336452c22e9740748bcc5160bfdd1d3e448fb239ead091bb654efb5b316fe915",
        current_sha384: "bc88b40603fb4854d7824435aeb305d89c8d13689c6e4b567f9579c881a8bb28d85e626ee938e38d255697ad24bca3ca",
    },
    HistoricalMigrationChecksum {
        version: 11,
        previous_sha384: "5bef26f22ad949be545f81e68b5480e6e727839a3e2bac452169e975b7dc1ee2fe9cae18eff6ae4e50b2b59c766a02cf",
        current_sha384: "f8eb39eb3a20aaf8caccaced472e5cd8d41217e26a3d423f1ddfdcb22f4014be32b79a8feeca374ba7910e866cf18b10",
    },
    HistoricalMigrationChecksum {
        version: 12,
        previous_sha384: "1e23cef384d27d8869eb58080e9383099efc44e5f1587fd01a84c32645a6c0453714804d4d5a855341fa018299a50d9e",
        current_sha384: "d28ba5f4f0ff1b8fd1c3d3ad64389c34721ed6112e24625089dd7ddbf9818f285da8321c3d77a29965de547c1511cb98",
    },
    HistoricalMigrationChecksum {
        version: 13,
        previous_sha384: "69e784e53d26571ae9bc58fdd5141b10164c39cb869601c53e2d019c97d6ada437060aade7b668706311205ee049637b",
        current_sha384: "2c922fc9fa69e1a6150e8473fce1c5de43c4a5e62e36a042149af06c76adb06bc0d02f3cbf7fcd0a239c6329dec7f135",
    },
    HistoricalMigrationChecksum {
        version: 14,
        previous_sha384: "1e72edd720453ce6985b8781520f4a8116dfb58dd6cf3aa13f055f21eb6157626ac7919d41852c1442c3e5fcaf6caea5",
        current_sha384: "a4018004257725f20952c208bf737d5d1b1f83261ef4c0ba53dc6c9ed23e58bf6061548dd8a70186187bf066019887e1",
    },
    HistoricalMigrationChecksum {
        version: 15,
        previous_sha384: "2d7c9bc733ddfbcaa89c0909da116f3ff6d1b089dc56152b2928164add97feee791aa338c272567c3abdf5fba2c40b38",
        current_sha384: "0cc03c58677fa979dc990a7ee4131c9e9d38956528e8b523c442875189616386d0f8924995fce142d74eff652519c321",
    },
    HistoricalMigrationChecksum {
        version: 16,
        previous_sha384: "1c75d4f2f6260561ed1de108de7d8114198fa89e3bc22e68a542f4fc02211fad8ffcca45e67e376ef3a29f510c0959f1",
        current_sha384: "17fd5ca8fa04f1cef32493fea66c262b664806ef03852c58f93166b2357d0cb7806bb92c7a483ce41d7922799849e2da",
    },
    HistoricalMigrationChecksum {
        version: 17,
        previous_sha384: "8a2ab20e01897d7db1fc53ccc8391cd4d9298c77276e2fba154adc617c05d54588667e08795e98c19df296f376ac1841",
        current_sha384: "78850862d653d901e2a65204926f06ca3b5a7c6240ce6cba13a4ef94b2878c2beefb25eabf666a099ef566a023f16991",
    },
    HistoricalMigrationChecksum {
        version: 18,
        previous_sha384: "67577438ed9156cb6065494490da8cb1b759312ca56496507c32f8ca2e5ebc381c8744e89662a7baf12d966830708fa6",
        current_sha384: "e36916f6ddca51177e25f08e399868aeb8b5c425d21e12d72c1d29b045d793cec6af1cd078da2f9b6ba4e41316e94bae",
    },
    HistoricalMigrationChecksum {
        version: 19,
        previous_sha384: "a1a4ed0998b1e936447ebc080de45cf20045b5416944e923fead83a2a24da3344052b92320192356be8988e3758c8b5f",
        current_sha384: "c4c2c845c20b422429093b324f623055be6ff56a553c2676826949f409876fdfd68ec79c227b0e7580b93dbcbbf58d57",
    },
    HistoricalMigrationChecksum {
        version: 20,
        previous_sha384: "04ddb97155053058c84c1cb82da6019006c5d3054c6b4276cd2f3c7ed1b2f4ee21ff733c1b82bde4071885c36c5e575f",
        current_sha384: "1c1c5c6722795b7946a5a21f1be4cbc8974333824fb7289e7cd4bd390e17eab5e7de0defeb068309644a702ba6b3e2e0",
    },
    HistoricalMigrationChecksum {
        version: 21,
        previous_sha384: "2447f8cc2544a3855f9abb484a58276710525cf42a2ba0ba53314dc463d49bb0c65018473e17e97acf0be17497678aa2",
        current_sha384: "507563a06565dc3cf46aadb354a6bb0d97becf958a17a7bbfbd51842b681d89828bd9ed00dda83d887e9842c92c2f409",
    },
    HistoricalMigrationChecksum {
        version: 22,
        previous_sha384: "a48622fc86a9d441460b867f524ea970109aab0940496c842ec1280fae03f8269b9f2b630d7757fcad8b2208afc746f9",
        current_sha384: "e7b9ef11244e55a507a36f61e003b5d1ff01f9a681249322cc93a36a588fd1481caaca7600cba2256ed8189f56bfe88d",
    },
    HistoricalMigrationChecksum {
        version: 23,
        previous_sha384: "aac370589e1077f2e5f7c11b499fb915d7ca8e07bad3c1a9084146af2ddac3cb3319b17fff0b4c4e442fe57e34522df5",
        current_sha384: "bc6f3f1506119d2d39c4addf601995b3d76a9778f09ae668fe3c3049cc8555c03215325522e1cf8d70e7417a663e5119",
    },
    HistoricalMigrationChecksum {
        version: 24,
        previous_sha384: "6840f860f5356c7ee387c9ace6e30d5788f0599fc849a83a42eea5b9eae3bc9b386c290a069f5ccab5f25484a378241e",
        current_sha384: "dafd36f7ed8b4518290f43782b63b1065435199198578e666962f97f3a9cd74eeb86cdd43cad92d02ea3f78bcb98226e",
    },
    HistoricalMigrationChecksum {
        version: 25,
        previous_sha384: "23d237de7a9cc110aba24a9af5bb4f25ac80f7f51989a8cda60b7fb00d7736a8da564d6a13080236eb8290cb73c371c9",
        current_sha384: "fcfd86d9c5ba0d71cae6b25405c3b22d97568154f65d63200bf4d17ae334f80a62ddb8b10da74c055f8e3607beb17de4",
    },
    HistoricalMigrationChecksum {
        version: 26,
        previous_sha384: "8dcbf30c7128b4ed921d73a593fe701814eb1901f7c21b7edbae07f802fb13679c045d393d5f14f38972ff81e72bf965",
        current_sha384: "91f2e721f97870177a9836346c98abdb7cf4cd25de1987a6fd17b1bdbbf2a7ea060755648fbb55c4783dabdbabf4ee96",
    },
    HistoricalMigrationChecksum {
        version: 27,
        previous_sha384: "5c4dbafaa4349bacc9cd1d5ee3d68c84daffd45eac47eb63909246dc5c5688b67da547da844086ea7f8654e56c5b0248",
        current_sha384: "5c4dbafaa4349bacc9cd1d5ee3d68c84daffd45eac47eb63909246dc5c5688b67da547da844086ea7f8654e56c5b0248",
    },
    HistoricalMigrationChecksum {
        version: 28,
        previous_sha384: "ee354ee0d112c95c79448452e3afdce1809b38ba96325a074f0e137722a5051d3b61b818c3a2da835bae0e0a3bcb0eba",
        current_sha384: "f29da467347634600b36d75d951caba3a195914a8e8d02f993629edb0d7879db49c1db4de58acfe67b067fdad9b6650f",
    },
    HistoricalMigrationChecksum {
        version: 29,
        previous_sha384: "60477845200d85ff555aba008f20eaaa993a4e677d604817f5cdeb1d94bd432511e360caa4b27d200a3d0e2461d54c83",
        current_sha384: "46540c44a5bbf87ecb902ec4f8addd9c4bd5cd0ad231db968b9d853678c267dd554da2aec7265a8836204c912bfd1bc4",
    },
    HistoricalMigrationChecksum {
        version: 30,
        previous_sha384: "25335094bba04f09307ad03a020487b4d3b3ad60637f725935ba0e2e99edaca0962441a675c3d9df64b9c07cebe87a3d",
        current_sha384: "9ef5e19ca3b2cad03b06c1e385c0a447a9aa1d8d087262fad424b4d580129951c99af7de3478c9accf8655727894eff3",
    },
    HistoricalMigrationChecksum {
        version: 31,
        previous_sha384: "16fb8e3d0e89a034f784f32c6943e60831461a41d3e0f2d038bcddedd6588977a61f9cadb87a0530092bd569526e10d9",
        current_sha384: "4e0a8ac708ee3abe4e612644f9708e70243f0141dec01101bf91c8c68ec72679f98da995c3aad10e39f3bd5c987b2383",
    },
    HistoricalMigrationChecksum {
        version: 32,
        previous_sha384: "d3e5866d5f6e4ffe743d8d349fe81250027c9bfee7b90a31b8b4237aed8753b6efa62f2380544ae93e42d5ce181d1a72",
        current_sha384: "01703edc8e1225e3f9d29f6e537fc8c7e7184dfc6075fa42bfadb3d02296e056cd13c8ae7380cb41d6341892be357c5d",
    },
    HistoricalMigrationChecksum {
        version: 33,
        previous_sha384: "b43a237ba5daed4b2fe63f9f9b4bc58644afe3404c0c7813ad5e4d22a6d83874f89651e0ea0c5775bbd5483a74895d65",
        current_sha384: "a03c4a2e6977982918a9733f5cd8e99b3dc69cd5070fba30bfa5761b678307aa562022ab49d459e72adaaee68b0a6120",
    },
    HistoricalMigrationChecksum {
        version: 34,
        previous_sha384: "8623eb89d4b7b4a9d328dd6ff8e3f210149026096bedc455d55cc7c76208b597e8887e6774e35b339d1d1ace16d08fb7",
        current_sha384: "4e44baf35878d2f48e52f0e00f828dbfba81c58f5c9c7cb6a53376f8838cc55d8ae08c281ef85f0e06993eba3a72628d",
    },
    HistoricalMigrationChecksum {
        version: 35,
        previous_sha384: "06fffed192073d87a35b75e09f943fb61242797a534891cc44c33c32368ed79c8ec2ffb7fff48752166d3daed700c7f7",
        current_sha384: "4bf5dd7caddc3cbe43c16674403c743caf05c0d467776f2a85627413f9f18799fd079670863e5c99633b942c5cee14cb",
    },
    HistoricalMigrationChecksum {
        version: 36,
        previous_sha384: "1f23152aac960f25527d7d9ba23e75a9fce1186a577f4acef569b993d55703eca3bce6024d63cec249399c374620cb8a",
        current_sha384: "270b36b3a580f9a7579ac98dc5988a98e053f9f0de9a77551f15f4f4df8f3274bcf2228b11e200eeb97f49e47124520a",
    },
    HistoricalMigrationChecksum {
        version: 37,
        previous_sha384: "daa91be14f492456717827d909075c6af877a677f1ea4bd46990683e48265b99c9b937b6483cb9354d6f1658a1d199d9",
        current_sha384: "9e33232768d9be10105e67d62fc456e44c220ed8ae125ad2a6fc3b3cf449040bdd07c0e5c9af4f7e32b21a3bb87e8078",
    },
    HistoricalMigrationChecksum {
        version: 38,
        previous_sha384: "38b6990dc72861d652895ca29a3a36fa383851e168f5afebafefd7fe13acccb0c55e525dd44d8a34b6ee1811b1c7c8b7",
        current_sha384: "43757c5126e24b8e7f38ecf8e0b674cdd61e700b00390ae12e9e60340d1ef0281cf0a06454e1675f3c4614c0d75589e1",
    },
    HistoricalMigrationChecksum {
        version: 39,
        previous_sha384: "bcaf9de69a598f9a4efd54ccaf1d533ff2b3e38515afe0e8be77c13b289f49613eaf3124cf359980efebbd368d3db37d",
        current_sha384: "05b499b6ccb0097b1f9c81bbc20cb27278c3e2404ff2c694a780e131a3c1f9e753f8f7bf98984b222bccb27f6b2d600d",
    },
    HistoricalMigrationChecksum {
        version: 40,
        previous_sha384: "77748d814a11be327dfbd0bf57d5bccb66a98542099890177f3031791ec3117fbe29e76ae2bae54bed017efdc1d731a3",
        current_sha384: "3de6486d626cb4311cf0fddad2891ffd6b1e2f9ab9347bc839f25294b6e163e414bc8c6f2d0d5128ccccce85ab29be23",
    },
    HistoricalMigrationChecksum {
        version: 41,
        previous_sha384: "1675bb97a35a2422bdc98eb4d3cee0d447afa491b1476040ec636dc4f8d265d194a8b9a40b0aa676f476fc8f98c9c54d",
        current_sha384: "602982602194b3d10703e451741678b7542dfd57bbdc992252f8201ce9a5a286632c527af9823f73a450ee861ca2fac9",
    },
    HistoricalMigrationChecksum {
        version: 42,
        previous_sha384: "189bf7c841f30c8b174be335478ceb06ef2e002aa1bc11b831b3a2ca9151c2eced3d2f53b4b4610b5d821d2e92c196cd",
        current_sha384: "8ac565c53cac5e75d96a5f868564e5cbc48a9c0eb03a13743e1dc79ea381ce50ccf273872bb5651e0d632e2aa221da14",
    },
    HistoricalMigrationChecksum {
        version: 43,
        previous_sha384: "85de191f1c82966cbde8390752f66b03194d340ecac8c48beae2946419f791e159e148e2a2747993831675cc811fc5cb",
        current_sha384: "71ac6538b72d4f4db1c7b9ae4899ecf99dc28627daef79474dc06ac3762b2de9dcfdeaa63e227810bfc5d8a587bead38",
    },
    HistoricalMigrationChecksum {
        version: 44,
        previous_sha384: "ee0e8e9eec97346928f6b6cc924c99fbe89cb5f26ca9d3d6113358914d70c74dd5c1ad9419dccbec6a7bd46cf212cd3e",
        current_sha384: "2e2c4daa45b5788e34a4f052a701b4c3db0e882849e84b0f09b5ba24a7d2a8c2707930c576aa41d668e5138295cec97f",
    },
    HistoricalMigrationChecksum {
        version: 45,
        previous_sha384: "983c52ea2264b77a6029536b79aa639a251e36da7f3bcd1bf40a3e78952783980134c99078145f7add6a44467fbd7d9f",
        current_sha384: "51039b5e133982a8ea9827701c7a39a9a46d13f8c58dee9a0dadfef87cf0eeedb2d74edc9e722343a1344df81c4f3267",
    },
    HistoricalMigrationChecksum {
        version: 46,
        previous_sha384: "2f572e386cc7754e938a14ff7c84acea5f0a6aab0528b7a1b0d54057cc7016eacdecde520daa6990571398c448f5bf03",
        current_sha384: "adc305335b41965a2ae7826f5bbac7b359a834830559e4a1f6df531806db1aeec3851c66d56a3b14a75782d810c0dac1",
    },
    HistoricalMigrationChecksum {
        version: 47,
        previous_sha384: "e59b3b5ee6b2c711dda9a3417456bb449a8c2b9d0ed08521ab4b44290675f5ea86dcc712d18cd46c81af10d90b260154",
        current_sha384: "d6730311973e14fb2f1193f952b12267763dd8fa54154464458a5321d6ff74b3b335bdfd909d93bf51b9da095aa3de6f",
    },
    HistoricalMigrationChecksum {
        version: 48,
        previous_sha384: "5a9fb89f1cf7cd11b22397d2a40970d2decbc45805105110eefb61125541bc68ee09c7ac37b1b43a501b8a14a0b7dcb2",
        current_sha384: "c660726b512fb750bed819321ab64e778ecba32a0ee824db4cdde9adeaaaaea17baf214fcb76baddb537e110cc786075",
    },
    HistoricalMigrationChecksum {
        version: 49,
        previous_sha384: "4c73ce24e825ff98c79341dd6b6ddb7a8b064cd598f988dbbebb339eda1333ea72e8ea85b90971049d6524af16dea820",
        current_sha384: "9792847bb1c38e616dcf0484e9d0b92bd2ef098e4b8f2393e7cb221e31e102d577e36a3acefc6c5538478d1dac8fb895",
    },
    HistoricalMigrationChecksum {
        version: 50,
        previous_sha384: "147fb95e6228e6957a027bbbd5aed8af264a3e6a80858833dbc11847212e70341bdae12f6a14ed0f68b605032261474f",
        current_sha384: "2db71d052e6c5d7786f78ac3c108fca25b0917a2d466138fc906e5052d9cae782ab58e8bda6a2653adcd2a663b6845fa",
    },
    HistoricalMigrationChecksum {
        version: 51,
        previous_sha384: "0b4c07a541378ab39a13fe8848247b5ccce65a673a562f3d8d2a5c23e14267e61647b26e529711a2468bc3e4456c56b6",
        current_sha384: "ac8925b18013ee401b86244cdebf47a31d675b89367c5ece3d39d597431604e9ce8d935088c313d54b49cb110d0c22ba",
    },
    HistoricalMigrationChecksum {
        version: 52,
        previous_sha384: "024e5bf6973d78c1b1be6d3197defba9781aa82c96d4397ceb00f86b16e24d6b383c5859b14f363e3f5d51332c12d860",
        current_sha384: "8b36c50dff691d1afa22e2b8e517d97c70744175a9e097284e37f4fe049607b4986f55cc19d9a26e4eac862ea4995e05",
    },
    HistoricalMigrationChecksum {
        version: 53,
        previous_sha384: "fa43654e0c9a6968492a54ab420c7c17cca9c1f62e95fb3dfde90ebf2610cafc4ecb978eef406fdcd74a7857129fdab4",
        current_sha384: "6810cbfa6e25c2b87ea1c2713b834a20b136686698cccbbc24e211a6d70d2544c75f999e7acca1f8f0439a0ec7ed6b2b",
    },
    HistoricalMigrationChecksum {
        version: 54,
        previous_sha384: "235f992ecdcef978a6fcd729af760948721e4e17b2d7fb9313a1ccf17bcd9955a4bd5e66f882eea223e77c894cd98736",
        current_sha384: "9434d539009c9b3f194680e3e4a87831af5e700173cbfcf52ad5e16ac278306f1de3b75cc18ee9eba9a3d7f5e1fe7de9",
    },
    HistoricalMigrationChecksum {
        version: 55,
        previous_sha384: "6ea68494b5bd1d1db51287f1d48aef23465cb9d766279d829a5b098be41ee71cedc0854a2356e22dd74f4e82c9755d2f",
        current_sha384: "d7a71dbd48c478a14cee1e3916b0828eefe2201b4c47014353e1e2c2d158444c1fe368bc47a897264bcaa3cca8fc7ef0",
    },
    HistoricalMigrationChecksum {
        version: 56,
        previous_sha384: "0acaa3616295cfca679c9dc1cf8fd14ca4bf6f55e6b1eae3bd11bb1c4ac46e517cbca2bc7edfdc1f313cc66efdc69b60",
        current_sha384: "1d8ea79d04cec6082c79cc6bf0b549ca4edf24cf5eb1cec2120b2c64744d8640b0b6bcc03a9d308b09b9387f6c76f101",
    },
    HistoricalMigrationChecksum {
        version: 57,
        previous_sha384: "798e8665162f198142f40323b740c824cb1f0dc9d7812abefc6afbb5181bef74a3d216aa80fb9ee669ee2508980bee36",
        current_sha384: "0e2c6b0b7e852a28891dd1c8b106bfe7fa9934b8f7e78a980873a121213f9ead30d908be261441e1a35c5bf4cada549e",
    },
    HistoricalMigrationChecksum {
        version: 58,
        previous_sha384: "0ec2f17be954f7dafa4f403d6ae3067f7ea8dd2d22917c634a0b84773c874a3b22e036f64dfe7b2b8a438e72e3cc1651",
        current_sha384: "37b2dbc6ca243a65c92baa3ee4175ec4281ba1dc86dee49364c9e7be1a30ba370b0a13fcf6914af82ce5c34bee0ee2c3",
    },
    HistoricalMigrationChecksum {
        version: 59,
        previous_sha384: "2236aa265912e5efbe610a8d554c1701f282c8bc2abdce737cdb0820b942166298c0a92c252b06ca0df4b743a04213ef",
        current_sha384: "878d7878132c28b0f5e3cf6fb9da5e06a92b57f9492df1c1a75fbf042312f87fd7c08663f9352e2888ccea048ba8be58",
    },
    HistoricalMigrationChecksum {
        version: 60,
        previous_sha384: "f2717ea35d9375b3b1843a0a83294f3d17accc11d9b2bb5928a45d5dd3e5b45da28e8f5a3e4832fcc0a738daf0245c51",
        current_sha384: "f44880587943428fc91643731e292a465f22a59b10bb7f199a38dfe3e8cbb95cd4bb0fbd4d96db72628bde233a89c7d3",
    },
    HistoricalMigrationChecksum {
        version: 61,
        previous_sha384: "e227cbd167a5cfcd22c04d0d23c56af6de79b8a161e7dee25abec577e1e5af97e122954208e930dc651a63d06f4c44db",
        current_sha384: "9f24a8879cc2fc19b14adc6dcc87c679d2333f51d41bb4e30be5667b64f527f72d6a04641615eff7a167f6ad27512b7d",
    },
    HistoricalMigrationChecksum {
        version: 62,
        previous_sha384: "867c5506428d14759bc913130826511834a027eebdfabe7d3865e5851abfc2a31e66d3d47f23b5049b27bb7b28459064",
        current_sha384: "9aa1336fdc0279ecc41e658cb21615bc142d06430de1b3bf33f7416fa03fc5cf309a261b07a3e164096f59c4aafe6879",
    },
    HistoricalMigrationChecksum {
        version: 63,
        previous_sha384: "cbbb5f51733b43132f6fd0c4b1609a6d6468749deec2e9d9762f7f1ca3a90000cc9c272308c5b71a1f2c40093cfa5ebf",
        current_sha384: "31738ab3ba95a9ed12ec5a369e2192db1f8a29139fa3dc18c962215380e378ca8c2fff4f0f2ca5aa2212d350f23005a3",
    },
    HistoricalMigrationChecksum {
        version: 64,
        previous_sha384: "507f83ebe2250253a020a43220893c85d9511a4e88fd4d9206550b50145b04c5a60f8a7bf1623b228753c629595b0de4",
        current_sha384: "1863bad2684f081fa9076107a7fd8c087740f72c7175b2717cd00384b8238779309f3d7edbcf8934a5a930758f7d36ba",
    },
    HistoricalMigrationChecksum {
        version: 65,
        previous_sha384: "411fa9106c0de484243df20e5d77cbed3f7d10b11ab9e39bf04c6187027c7eb503289b8cf4d34fb7a5b0662f4707ed20",
        current_sha384: "2d4b992f7bc9ba73e736f46327b15c10eeb6a57776e2fa8225aa5a609d8c1b31cb510c4ce9c3c735ed645ba64828b2b7",
    },
    HistoricalMigrationChecksum {
        version: 66,
        previous_sha384: "087a1fa7461bed14d1880dcc5c65ca1ef7e2e9aada3148007d39030c3be0d89b78ed838ed863d727a823c5b05f1aad98",
        current_sha384: "97399f76b3cebb0e3579f54c7cfa95395bacf54180d659da98cdf8212315a3608830421b4603e062bb63a585eb010da6",
    },
    HistoricalMigrationChecksum {
        version: 67,
        previous_sha384: "a0b9db598b3acca3f9bf719ffa019feac5398020df0451f2ab094ef61b33b2d331b038be0452024e3f36e44a60e3edf0",
        current_sha384: "987e7fea63818b1ca89b2d99683485badc633b9a20df787caaa6b49e6c8c875ed43923fdb8fba728fd8b7d97af5714a8",
    },
    HistoricalMigrationChecksum {
        version: 68,
        previous_sha384: "961dfdb2ac1fec9c99174e6b73aec3db58a4cc4f22cbcf73ce2b4cb784880debf10ef45ae420291417f8dbf6b5846cc1",
        current_sha384: "60e791b7bd1e383712b5d499a86a74908e0261aa54aba1d2f607c9aeea2892ab5502da83b77faba699b303ac8c429839",
    },
    HistoricalMigrationChecksum {
        version: 69,
        previous_sha384: "d4b18dd21343cceb6c32bb443d223d8d39a48fad1f8652d700bbe5cb303cd2028822e836fed75889d635139d404ec5d2",
        current_sha384: "60fca8f0ce9ba000376d8e637eb5e26eea024b7cfb53849bf4217680bfd63ff0e611163f5663349edc3ec067a1c3598c",
    },
    HistoricalMigrationChecksum {
        version: 70,
        previous_sha384: "f7cf912aad1ee4105813417072d994c017528ec879a7ea35f6e58a8c2c06684048828a15ba49541f78cd3658757caf10",
        current_sha384: "82600022ae54f17b6c52332478c9558e6597bfb88b10b91bf4c2d9622f95fd651db9e262a861f6d34f1d0933f26f9e8a",
    },
    HistoricalMigrationChecksum {
        version: 71,
        previous_sha384: "5c1aaf27e3c7eb98898d7898769538216823d330fedc7319eb016c67a5820d4c7c7b5cfdff5eac800b3b3dbf7139a834",
        current_sha384: "42fb6d9ad7ba4bbcfff980a4b2f49dd192f646b6d34647029c9fd46400ab99f00ce9f949619de553f6d1270a3c6f2c89",
    },
    HistoricalMigrationChecksum {
        version: 72,
        previous_sha384: "f1d435a4918dc38a573e137b00f04e87109725ce4ac32f4b4ecd97cea4d9f6c709171536bb7bab1ca7fef8af77069c84",
        current_sha384: "04e695337758bebd6c41a59398397d0561355973fb225fa120392775be805a4eb6c330c255a786fc8c5548fccc2fa960",
    },
    HistoricalMigrationChecksum {
        version: 73,
        previous_sha384: "b0efcf8b7bbaed5a557171c65c8bd36f69aebdd56bc496d75ada40f6ffc41a7c1470fb4e822a2ec1da92debb88cb19a3",
        current_sha384: "c84ed6f3fbb8bec2f895d868638d89a764239e56b98d8e26616f645b9da12a7c50a483aec55bcea7a148ab4610688f02",
    },
    HistoricalMigrationChecksum {
        version: 74,
        previous_sha384: "47f6b9fb2cdd158d210c418a0e3b06345cb398f61d6e12ea99cc1d67a31ce0c74897dc1e5488f95619c5060fc7c99276",
        current_sha384: "48c84ec0a4471c37588642413f2aebc369c7cd7f5e0bf0e915a6fede382d26b71b9c025c367221e4d501be80ec6a4fee",
    },
    HistoricalMigrationChecksum {
        version: 75,
        previous_sha384: "15a04b0b1e8429c31d6852ddae693105a3b20e5f393e7620e355e88640938f861f830a916819efca4ff0b8b752271dbe",
        current_sha384: "b1422b3a15365da96903dec8309bb97c1c7e122590ef068e8f4cf01174bf5db1670386d3e7c69794278071ba7f5ec036",
    },
    HistoricalMigrationChecksum {
        version: 76,
        previous_sha384: "545975b1c68fb3ff1c0fab65a1dd6008c41e5a70cee0c678f0260d74f39ac8d57ff0cb7519158c37f002f152a9e9ecb1",
        current_sha384: "ed7593542acbc0d31e734e56a71a69e805b3da9bd73d1fabed78a124384fc0d13bc0a5d171553ab53c550bcc2d4c8c99",
    },
    HistoricalMigrationChecksum {
        version: 77,
        previous_sha384: "28b1c6cd7a5dd4ac41e0d7754de7278a0da7d3e2a5d78075d9d5c03621dcf53b2b05169e594b54e491305d92579e5736",
        current_sha384: "e642f21ec1cbd0b511fc1276710839db870a284e152788204a13c5c97148221b0b81684bee816454c7f717c9e407b8b1",
    },
    HistoricalMigrationChecksum {
        version: 78,
        previous_sha384: "a624125eb2262ffc540878d024017042f039b5d294d8153f2852061c799ac7c27790faf610375ea7c7a3e06952b214bd",
        current_sha384: "fce26cf0f7ed2116e41ce6be86e746cf9615d9a2cd7b881e4f5ee6a72934be2e0ffe08aad1330c2802f37cca000fd385",
    },
    HistoricalMigrationChecksum {
        version: 79,
        previous_sha384: "96c073616b43ffe538a3dd36064c69437b71227ed6948807c84ae0c53529974db7f4e64be21b76de138fcb52503561a1",
        current_sha384: "3901ea55ba660212f58416ca99b3fc09dbf56e12aed4ff85399cde92d202ece805d02850ab8a40912c5b9e034d809a3a",
    },
    HistoricalMigrationChecksum {
        version: 80,
        previous_sha384: "73cbf18606060514799c64238176ad5d84790f6ec98ff2cfafeb55558a1ab593dd1b4724cf9823c5f30ceb9176180c66",
        current_sha384: "dfa39290416bf6b52167d44b66155fcb7a1c652701787e0807645c150357929d1ba2cfa8dd3e3387fdde47e9788c6130",
    },
    HistoricalMigrationChecksum {
        version: 81,
        previous_sha384: "a1d4b313d76c7d07c51871909326f6cc85432bb1987a42202eb2c89ec1cd5531fc52fc0ace346fa194e2eb41ae7bcca7",
        current_sha384: "2d82ba29ea5b2fc2d54feb838dcdc95370da6ddfe0d13b11d641f92f6956503eef3c3987e475c52af8feb71b9d04b0c4",
    },
    HistoricalMigrationChecksum {
        version: 82,
        previous_sha384: "df5f954cb54911bf109a30374bb849ee23bff213dd0d42819f22eaf79f6e989e608c1febf8a5a7ebb2d5a24c84adce2c",
        current_sha384: "2636c01eabd7be0fe4d9ced28fb51db26e3aa6d71978f5280c14b6cfa9d79a3ed4ec06fdd8023c0d76ad147a121e2ac1",
    },
];
