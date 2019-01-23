# stressTest
stress test for any website

TO RUN the solution replace `URL` with your site
*)- use `URL=URL node index.js`

*) `docker build -t stress-web --build-arg SITE_URL=URL .`

*) `docker run -itd -label stress-web:latest -P 4000:4000` 

check your `localhost:4000`
